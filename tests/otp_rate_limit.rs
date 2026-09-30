//! Coverage for the three rate-limiting tiers in `services/otp.rs` — resend
//! cooldown (60s), hourly cap (5 fresh challenges/hour), and max attempts (5)
//! — plus single-use challenges. A regression in any of these is either an
//! OTP bypass or paid-SMS spam, so each tier is pinned down end to end.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use common::{extract_otp_code, send, state};

async fn app_and_db() -> (axum::Router, PgPool) {
    let state = state().await;
    let db = state.db.clone();
    (aframp::router(state), db)
}

/// A fresh, collision-free signup body plus the E.164 phone the app stores
/// (and the mock OTP provider records messages under).
fn fresh_signup(seed: &str) -> (Value, String) {
    let unique = Uuid::new_v4();
    let local_digits = (unique.as_u128() as u64) % 10_000_000_000;
    let body = json!({
        "email": format!("{seed}+{}@example.com", unique.simple()),
        "password": "password123",
        "name": "Rate Limit",
        "phone_number": format!("0{local_digits:010}"),
    });
    (body, format!("+234{local_digits:010}"))
}

async fn signup(app: &axum::Router, body: &Value) -> (StatusCode, Value) {
    send(app.clone(), "POST", "/signup", None, Some(body.clone())).await
}

async fn verify(app: &axum::Router, challenge_id: &str, code: &str) -> (StatusCode, Value) {
    send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge_id, "code": code })),
    )
    .await
}

fn last_code(phone: &str) -> String {
    extract_otp_code(
        &aframp::otp::mock::last_message_for(phone)
            .expect("mock otp provider should have recorded a message for this phone"),
    )
}

/// A code guaranteed not to match `real` — so a "wrong attempt" can never
/// accidentally be right.
fn wrong_code(real: &str) -> &'static str {
    if real == "000000" {
        "111111"
    } else {
        "000000"
    }
}

async fn attempts(db: &PgPool, challenge_id: &str) -> i32 {
    sqlx::query_scalar("SELECT attempts FROM otp_challenges WHERE id = $1::uuid")
        .bind(challenge_id)
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn second_signup_within_cooldown_returns_429() {
    let (app, db) = app_and_db().await;
    let (body, phone) = fresh_signup("otp_cooldown");

    let (status, first) = signup(&app, &body).await;
    assert_eq!(status, StatusCode::OK, "first signup failed: {first}");
    let first_code = last_code(&phone);

    let (status, second) = signup(&app, &body).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{second}");
    assert_eq!(second["code"], "TOO_MANY_REQUESTS");

    // The rejected resend must not have sent a new SMS or rotated the code.
    assert_eq!(last_code(&phone), first_code, "a rate-limited resend must not send an SMS");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM otp_challenges WHERE phone_number = $1")
        .bind(&phone)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 1, "a rate-limited resend must not create a challenge");
}

#[tokio::test]
async fn sixth_signup_within_an_hour_returns_429() {
    let (app, db) = app_and_db().await;
    let (body, phone) = fresh_signup("otp_hourly");

    // Five fresh challenges, each driven through the real endpoint. Expiring
    // each one after it's created stops the next signup from simply
    // refreshing it in place — forcing a brand-new challenge every time,
    // which is exactly what the hourly cap counts.
    for n in 1..=5 {
        let (status, json) = signup(&app, &body).await;
        assert_eq!(status, StatusCode::OK, "signup #{n} should be allowed: {json}");
        sqlx::query("UPDATE otp_challenges SET expires_at = now() - interval '1 second' WHERE id = $1::uuid")
            .bind(json["challenge_id"].as_str().unwrap())
            .execute(&db)
            .await
            .unwrap();
    }

    let (status, json) = signup(&app, &body).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "sixth signup in an hour must be capped: {json}");
    assert_eq!(json["code"], "TOO_MANY_REQUESTS");

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM otp_challenges WHERE phone_number = $1")
        .bind(&phone)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 5, "the capped request must not insert a sixth challenge");
}

#[tokio::test]
async fn sixth_wrong_otp_attempt_returns_otp_locked() {
    let (app, db) = app_and_db().await;
    let (body, phone) = fresh_signup("otp_locked");

    let (status, challenge) = signup(&app, &body).await;
    assert_eq!(status, StatusCode::OK, "{challenge}");
    let challenge_id = challenge["challenge_id"].as_str().unwrap();
    let wrong = wrong_code(&last_code(&phone));

    for n in 1..=5 {
        let (status, json) = verify(&app, challenge_id, wrong).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "attempt #{n}: {json}");
        assert_eq!(json["code"], "OTP_INVALID", "attempt #{n} should be a plain wrong code");
    }
    assert_eq!(attempts(&db, challenge_id).await, 5);

    let (status, json) = verify(&app, challenge_id, wrong).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["code"], "OTP_LOCKED", "the sixth wrong attempt must hit the lock");

    // Locked means locked: the counter doesn't keep climbing, and the
    // correct code is refused too.
    assert_eq!(attempts(&db, challenge_id).await, 5);
    let (status, json) = verify(&app, challenge_id, &last_code(&phone)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["code"], "OTP_LOCKED");
}

#[tokio::test]
async fn resend_after_cooldown_succeeds_and_resets_attempt_counter() {
    let (app, db) = app_and_db().await;
    let (body, phone) = fresh_signup("otp_resend_reset");

    let (status, first) = signup(&app, &body).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let challenge_id = first["challenge_id"].as_str().unwrap().to_string();
    let wrong = wrong_code(&last_code(&phone));

    // Burn every attempt so the challenge is locked.
    for _ in 0..5 {
        verify(&app, &challenge_id, wrong).await;
    }
    let (_, json) = verify(&app, &challenge_id, wrong).await;
    assert_eq!(json["code"], "OTP_LOCKED");

    // Fast-forward past the 60s cooldown and resend.
    sqlx::query("UPDATE otp_challenges SET last_sent_at = now() - interval '61 seconds' WHERE id = $1::uuid")
        .bind(&challenge_id)
        .execute(&db)
        .await
        .unwrap();

    let (status, second) = signup(&app, &body).await;
    assert_eq!(status, StatusCode::OK, "resend after cooldown should succeed: {second}");
    assert_eq!(
        second["challenge_id"].as_str().unwrap(),
        challenge_id,
        "a resend refreshes the existing challenge rather than creating another"
    );
    assert_eq!(attempts(&db, &challenge_id).await, 0, "resend must reset the attempt counter");

    // The reset is real: the fresh code now verifies despite the earlier lock.
    let (status, json) = verify(&app, &challenge_id, &last_code(&phone)).await;
    assert_eq!(status, StatusCode::OK, "fresh code should verify after resend: {json}");
    assert!(json["token"].as_str().is_some());
}

#[tokio::test]
async fn consumed_challenge_cannot_be_reused() {
    let (app, db) = app_and_db().await;
    let (body, phone) = fresh_signup("otp_single_use");

    let (status, challenge) = signup(&app, &body).await;
    assert_eq!(status, StatusCode::OK, "{challenge}");
    let challenge_id = challenge["challenge_id"].as_str().unwrap();
    let code = last_code(&phone);

    let (status, json) = verify(&app, challenge_id, &code).await;
    assert_eq!(status, StatusCode::OK, "first verify should succeed: {json}");

    let consumed: bool = sqlx::query_scalar("SELECT consumed_at IS NOT NULL FROM otp_challenges WHERE id = $1::uuid")
        .bind(challenge_id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert!(consumed, "a verified challenge must be marked consumed");

    // Replaying the exact same, previously-valid code must not mint a second session.
    let (status, json) = verify(&app, challenge_id, &code).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "replay must be rejected: {json}");
    assert_eq!(json["code"], "OTP_CHALLENGE_NOT_FOUND");
    assert!(json.get("token").is_none(), "replay must not issue a token");
}
