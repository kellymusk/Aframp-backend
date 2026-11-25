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
        "password": "Password123!",
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

/// Inserts a challenge row directly, bypassing the endpoint, so tests can
/// control `created_at`, `expires_at`, and `consumed_at` precisely.
async fn insert_challenge(
    db: &PgPool,
    phone: &str,
    created_at: &str,
    expires_at: &str,
    consumed_at: Option<&str>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO otp_challenges \
         (id, phone_number, code_hash, attempts, created_at, expires_at, last_sent_at, consumed_at) \
         VALUES ($1::uuid, $2, 'hash', 0, now() - $3::interval, now() - $4::interval, now() - $3::interval, \
                 CASE WHEN $5::text IS NULL THEN NULL ELSE now() - $5::interval END)",
    )
    .bind(id)
    .bind(phone)
    .bind(created_at)
    .bind(expires_at)
    .bind(consumed_at)
    .execute(db)
    .await
    .unwrap();
    id
}

async fn exists(db: &PgPool, id: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM otp_challenges WHERE id = $1::uuid)")
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn purge_removes_consumed_challenge_older_than_24h() {
    let (_app, db) = app_and_db().await;
    let phone = format!("+234{}", (Uuid::new_v4().as_u128() as u64) % 10_000_000_000);

    let id = insert_challenge(
        &db,
        &phone,
        "25 hours",
        "1 hour",
        Some("24 hours"),
    )
    .await;
    assert!(exists(&db, id).await, "row should exist before purge");

    aframp::services::otp::purge_stale_challenges(&db)
        .await
        .expect("purge_stale_challenges should succeed");

    assert!(
        !exists(&db, id).await,
        "a consumed challenge older than 24h must be purged"
    );
}

#[tokio::test]
async fn purge_keeps_unconsumed_unexpired_challenge_older_than_24h() {
    let (_app, db) = app_and_db().await;
    let phone = format!("+234{}", (Uuid::new_v4().as_u128() as u64) % 10_000_000_000);

    // Created 25h ago but still valid for another hour and never consumed.
    let id = insert_challenge(&db, &phone, "25 hours", "-1 hour", None).await;

    aframp::services::otp::purge_stale_challenges(&db)
        .await
        .expect("purge_stale_challenges should succeed");

    assert!(
        exists(&db, id).await,
        "an unconsumed, non-expired challenge must not be purged"
    );
}

#[tokio::test]
async fn purge_removes_challenge_expired_older_than_24h() {
    let (_app, db) = app_and_db().await;
    let phone = format!("+234{}", (Uuid::new_v4().as_u128() as u64) % 10_000_000_000);

    // Expired 25h ago and never consumed.
    let id = insert_challenge(&db, &phone, "26 hours", "25 hours", None).await;

    aframp::services::otp::purge_stale_challenges(&db)
        .await
        .expect("purge_stale_challenges should succeed");

    assert!(
        !exists(&db, id).await,
        "a challenge expired more than 24h ago must be purged"
    );
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
    assert_eq!(
        last_code(&phone),
        first_code,
        "a rate-limited resend must not send an SMS"
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM otp_challenges WHERE phone_number = $1")
            .bind(&phone)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(
        count, 1,
        "a rate-limited resend must not create a challenge"
    );
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
        assert_eq!(
            status,
            StatusCode::OK,
            "signup #{n} should be allowed: {json}"
        );
        sqlx::query("UPDATE otp_challenges SET expires_at = now() - interval '1 second' WHERE id = $1::uuid")
            .bind(json["challenge_id"].as_str().unwrap())
            .execute(&db)
            .await
            .unwrap();
    }

    let (status, json) = signup(&app, &body).await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "sixth signup in an hour must be capped: {json}"
    );
    assert_eq!(json["code"], "TOO_MANY_REQUESTS");

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM otp_challenges WHERE phone_number = $1")
            .bind(&phone)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(
        count, 5,
        "the capped request must not insert a sixth challenge"
    );
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
        assert_eq!(
            json["code"], "OTP_INVALID",
            "attempt #{n} should be a plain wrong code"
        );
    }
    assert_eq!(attempts(&db, challenge_id).await, 5);

    let (status, json) = verify(&app, challenge_id, wrong).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(
        json["code"], "OTP_LOCKED",
        "the sixth wrong attempt must hit the lock"
    );

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
    assert_eq!(
        status,
        StatusCode::OK,
        "resend after cooldown should succeed: {second}"
    );
    assert_eq!(
        second["challenge_id"].as_str().unwrap(),
        challenge_id,
        "a resend refreshes the existing challenge rather than creating another"
    );
    assert_eq!(
        attempts(&db, &challenge_id).await,
        0,
        "resend must reset the attempt counter"
    );

    // The reset is real: the fresh code now verifies despite the earlier lock.
    let (status, json) = verify(&app, &challenge_id, &last_code(&phone)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "fresh code should verify after resend: {json}"
    );
    assert!(json["token"].as_str().is_some());
}

// ---------------------------------------------------------------------------
// #1486 — explicit coverage for each `upsert_challenge` branch.
//
// `upsert_challenge` dispatches to one of three paths:
//   1. existing challenge within cooldown  -> reject (429, no row, no SMS)
//   2. existing challenge past cooldown    -> refresh in place (same row id)
//   3. no existing challenge               -> insert a brand-new row
// The tests below pin each branch down so the proposed split into
// `refresh_existing_challenge` / `insert_new_challenge` cannot drift.
// ---------------------------------------------------------------------------

/// Branch 3: no existing challenge -> a fresh row is inserted.
#[tokio::test]
async fn upsert_inserts_new_challenge_when_none_exists() {
    let (app, db) = app_and_db().await;
    let (body, phone) = fresh_signup("otp_branch_insert");

    let (status, json) = signup(&app, &body).await;
    assert_eq!(status, StatusCode::OK, "first signup failed: {json}");
    let challenge_id = json["challenge_id"].as_str().unwrap();

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM otp_challenges WHERE phone_number = $1")
            .bind(&phone)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(count, 1, "a fresh signup must insert exactly one challenge");

    let (attempts, consumed, expired): (i32, bool, bool) = sqlx::query_as(
        "SELECT attempts, consumed_at IS NOT NULL, expires_at <= now() \
         FROM otp_challenges WHERE id = $1::uuid",
    )
    .bind(challenge_id)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(attempts, 0, "a new challenge starts with zero attempts");
    assert!(!consumed, "a new challenge is not consumed");
    assert!(!expired, "a new challenge is not expired");

    // The inserted challenge is usable end to end.
    let (status, json) = verify(&app, challenge_id, &last_code(&phone)).await;
    assert_eq!(status, StatusCode::OK, "fresh code should verify: {json}");
}

/// Branch 1: existing challenge within cooldown -> rejected, nothing changes.
#[tokio::test]
async fn upsert_rejects_existing_challenge_within_cooldown() {
    let (app, db) = app_and_db().await;
    let (body, phone) = fresh_signup("otp_branch_cooldown");

    let (status, first) = signup(&app, &body).await;
    assert_eq!(status, StatusCode::OK, "first signup failed: {first}");
    let challenge_id = first["challenge_id"].as_str().unwrap().to_string();
    let first_code = last_code(&phone);

    // Snapshot the row so we can prove the rejected resend left it untouched.
    let before: (String, String, String) = sqlx::query_as(
        "SELECT code_hash, last_sent_at::text, expires_at::text \
         FROM otp_challenges WHERE id = $1::uuid",
    )
    .bind(&challenge_id)
    .fetch_one(&db)
    .await
    .unwrap();

    let (status, second) = signup(&app, &body).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{second}");
    assert_eq!(second["code"], "TOO_MANY_REQUESTS");

    let after: (String, String, String) = sqlx::query_as(
        "SELECT code_hash, last_sent_at::text, expires_at::text \
         FROM otp_challenges WHERE id = $1::uuid",
    )
    .bind(&challenge_id)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(
        before, after,
        "a cooldown rejection must not touch the existing challenge"
    );
    assert_eq!(
        last_code(&phone),
        first_code,
        "a cooldown rejection must not send a new SMS"
    );

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM otp_challenges WHERE phone_number = $1")
            .bind(&phone)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(count, 1, "a cooldown rejection must not insert a new row");
}

/// Branch 2: existing challenge past cooldown -> refreshed in place.
#[tokio::test]
async fn upsert_refreshes_existing_challenge_past_cooldown() {
    let (app, db) = app_and_db().await;
    let (body, phone) = fresh_signup("otp_branch_refresh");

    let (status, first) = signup(&app, &body).await;
    assert_eq!(status, StatusCode::OK, "first signup failed: {first}");
    let challenge_id = first["challenge_id"].as_str().unwrap().to_string();
    let first_code = last_code(&phone);

    let before: (String, String) = sqlx::query_as(
        "SELECT code_hash, expires_at::text FROM otp_challenges WHERE id = $1::uuid",
    )
    .bind(&challenge_id)
    .fetch_one(&db)
    .await
    .unwrap();

    // Fast-forward past the 60s cooldown.
    sqlx::query("UPDATE otp_challenges SET last_sent_at = now() - interval '61 seconds' WHERE id = $1::uuid")
        .bind(&challenge_id)
        .execute(&db)
        .await
        .unwrap();

    let (status, second) = signup(&app, &body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "resend after cooldown should succeed: {second}"
    );
    assert_eq!(
        second["challenge_id"].as_str().unwrap(),
        challenge_id,
        "a refresh must reuse the existing row id"
    );

    let after: (String, String) = sqlx::query_as(
        "SELECT code_hash, expires_at::text FROM otp_challenges WHERE id = $1::uuid",
    )
    .bind(&challenge_id)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_ne!(
        before.0, after.0,
        "a refresh must rotate the stored code hash"
    );
    assert_ne!(
        before.1, after.1,
        "a refresh must extend the expiry"
    );
    assert_ne!(
        last_code(&phone),
        first_code,
        "a refresh must send a new SMS"
    );

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM otp_challenges WHERE phone_number = $1")
            .bind(&phone)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(count, 1, "a refresh must not insert a second row");
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

    let (status, json) = verify(&app, challenge_id, &code).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(
        json["code"], "OTP_INVALID",
        "a consumed challenge must not be reusable"
    );

    let consumed: bool = sqlx::query_scalar(
        "SELECT consumed_at IS NOT NULL FROM otp_challenges WHERE id = $1::uuid",
    )
    .bind(challenge_id)
    .fetch_one(&db)
    .await
    .unwrap();
    assert!(consumed, "a successful verify must mark the challenge consumed");
}
