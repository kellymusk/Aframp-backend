mod common;

use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

use common::{extract_otp_code, send, send_with_cookie, state};

async fn app() -> Option<axum::Router> {
    state().await.map(aframp::router)
}

/// Like `app()`, but also hands back a raw `PgPool` for tests that need to
/// force an OTP challenge into a particular state (expired, stale) directly.
async fn app_and_db() -> Option<(axum::Router, sqlx::PgPool)> {
    let state = common::state().await?;
    let db = state.db.clone();
    Some((aframp::router(state), db))
}

/// A fresh, collision-free (email, local phone form, E.164 phone form) triple.
fn fresh_identity(seed: &str) -> (String, String, String) {
    let unique = Uuid::new_v4();
    let email = format!("{seed}+{}@example.com", unique.simple());
    let local_digits = (unique.as_u128() as u64) % 10_000_000_000;
    let phone_number = format!("0{local_digits:010}");
    let normalized_phone = format!("+234{local_digits:010}");
    (email, phone_number, normalized_phone)
}

#[tokio::test]
async fn signup_then_verify_otp_issues_session() {
    let Some(app) = app().await else {
        return;
    };
    let (email, phone_number, normalized_phone) = fresh_identity("alice");

    let (status, challenge) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": email, "password": "password123", "name": "Alice", "phone_number": phone_number })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "signup failed: {challenge}");
    assert!(challenge.get("token").is_none(), "signup must not issue a session before OTP verification");
    let challenge_id = challenge["challenge_id"].as_str().unwrap();
    assert!(challenge["expires_in_secs"].as_i64().unwrap() > 0);

    let message = aframp::otp::mock::last_message_for(&normalized_phone).expect("mock provider should record the message");
    let code = extract_otp_code(&message);

    let (status, verified) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge_id, "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "verify-otp failed: {verified}");
    assert!(verified["token"].as_str().unwrap().len() > 10);
    assert!(verified["merchant_id"].as_str().is_some());
}

#[tokio::test]
async fn signup_pending_same_email_is_not_a_conflict_until_verified() {
    let Some(app) = app().await else {
        return;
    };
    let (email, phone_number, _) = fresh_identity("pending");

    let body = json!({ "email": email, "password": "password123", "name": "Pending", "phone_number": phone_number });
    let (status1, first) = send(app.clone(), "POST", "/signup", None, Some(body.clone())).await;
    assert_eq!(status1, StatusCode::OK, "first signup failed: {first}");

    // Neither challenge has been verified — resubmitting is a resend, not a conflict.
    let (status2, second) = send(app.clone(), "POST", "/signup", None, Some(body)).await;
    assert_eq!(status2, StatusCode::TOO_MANY_REQUESTS, "immediate resend should hit the cooldown, not a conflict: {second}");
}

#[tokio::test]
async fn signup_duplicate_email_conflicts_after_verification() {
    let Some(app) = app().await else {
        return;
    };
    let (email, phone_number, normalized_phone) = fresh_identity("dup");
    let body = json!({ "email": email, "password": "password123", "name": "Dup", "phone_number": phone_number });

    let (status, challenge) = send(app.clone(), "POST", "/signup", None, Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK, "signup failed: {challenge}");
    let code = extract_otp_code(&aframp::otp::mock::last_message_for(&normalized_phone).unwrap());
    let (status, _) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge["challenge_id"], "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A fresh signup attempt with the same (now real) email fails at the pre-check.
    let (_, other_phone, _) = fresh_identity("dup2");
    let (status, conflict) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": email, "password": "password123", "name": "Dup", "phone_number": other_phone })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "expected conflict: {conflict}");
    assert_eq!(conflict["code"], "EMAIL_TAKEN");
}

#[tokio::test]
async fn signup_weak_password_rejected() {
    let Some(app) = app().await else {
        return;
    };
    let (_, phone_number, _) = fresh_identity("weak");
    let (status, _) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": "weak@example.com", "password": "short", "name": "Weak", "phone_number": phone_number })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn signup_invalid_phone_rejected() {
    let Some(app) = app().await else {
        return;
    };
    let (email, _, _) = fresh_identity("badphone");
    let (status, body) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": email, "password": "password123", "name": "Bad Phone", "phone_number": "123" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["field"], "phone_number");
}

async fn signup_and_verify(app: &axum::Router, seed: &str) -> (String, String, serde_json::Value) {
    let (email, phone_number, normalized_phone) = fresh_identity(seed);
    let (status, challenge) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": email, "password": "password123", "name": "Test User", "phone_number": phone_number })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "signup failed: {challenge}");
    let code = extract_otp_code(&aframp::otp::mock::last_message_for(&normalized_phone).unwrap());
    let (status, verified) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge["challenge_id"], "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "verify-otp failed: {verified}");
    (email, normalized_phone, verified)
}

#[tokio::test]
async fn login_with_verified_phone_requires_otp() {
    let Some(app) = app().await else {
        return;
    };
    let (email, normalized_phone, _) = signup_and_verify(&app, "loginotp").await;

    let (status, challenge) = send(
        app.clone(),
        "POST",
        "/login",
        None,
        Some(json!({ "email": email, "password": "password123" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login failed: {challenge}");
    assert!(challenge.get("token").is_none(), "login for a phone-verified account must not issue a session directly");
    let challenge_id = challenge["challenge_id"].as_str().expect("login should return a fresh challenge_id");

    let code = extract_otp_code(&aframp::otp::mock::last_message_for(&normalized_phone).unwrap());
    let (status, verified) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge_id, "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "verify-otp failed: {verified}");
    assert!(verified["token"].as_str().unwrap().len() > 10);
}

#[tokio::test]
async fn login_legacy_account_without_phone_skips_otp() {
    let Some((app, db)) = app_and_db().await else {
        return;
    };
    // Simulate a pre-migration row: the API can no longer produce one, since
    // every signup now requires a phone.
    let email = format!("legacy+{}@example.com", Uuid::new_v4().simple());
    let real_hash = aframp_password_hash_for_tests();
    sqlx::query("INSERT INTO users (email, password_hash, name) VALUES ($1, $2, 'Legacy User')")
        .bind(&email)
        .bind(&real_hash)
        .execute(&db)
        .await
        .unwrap();

    let (status, body) = send(
        app.clone(),
        "POST",
        "/login",
        None,
        Some(json!({ "email": email, "password": "legacy-password-123" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "legacy login should succeed synchronously: {body}");
    assert!(body["token"].as_str().unwrap().len() > 10, "legacy login must issue a session directly, no OTP");
}

/// A real Argon2 hash of `legacy-password-123`, computed once so the legacy
/// test above can log in with a password this test suite actually knows.
fn aframp_password_hash_for_tests() -> String {
    use argon2::password_hash::{PasswordHasher, SaltString};
    use argon2::Argon2;
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    Argon2::default()
        .hash_password("legacy-password-123".as_bytes(), &salt)
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn login_wrong_password_unauthorized() {
    let Some(app) = app().await else {
        return;
    };
    let (email, _, _) = signup_and_verify(&app, "wrongpw").await;

    let (status, _) = send(
        app.clone(),
        "POST",
        "/login",
        None,
        Some(json!({ "email": email, "password": "not-the-password" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn verify_otp_wrong_code_rejected() {
    let Some(app) = app().await else {
        return;
    };
    let (email, phone_number, _) = fresh_identity("wrongcode");
    let (_, challenge) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": email, "password": "password123", "name": "Wrong Code", "phone_number": phone_number })),
    )
    .await;

    let (status, body) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge["challenge_id"], "code": "000000" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "OTP_INVALID");
}

#[tokio::test]
async fn verify_otp_unknown_challenge_404() {
    let Some(app) = app().await else {
        return;
    };
    let (status, body) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": Uuid::new_v4().to_string(), "code": "123456" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "OTP_CHALLENGE_NOT_FOUND");
}

#[tokio::test]
async fn otp_attempts_exhausted_locks_challenge() {
    let Some(app) = app().await else {
        return;
    };
    let (email, phone_number, normalized_phone) = fresh_identity("locked");
    let (_, challenge) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": email, "password": "password123", "name": "Locked", "phone_number": phone_number })),
    )
    .await;
    let challenge_id = challenge["challenge_id"].as_str().unwrap();

    for _ in 0..5 {
        let (status, _) = send(
            app.clone(),
            "POST",
            "/verify-otp",
            None,
            Some(json!({ "challenge_id": challenge_id, "code": "000000" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // Even the real code no longer works — the challenge is dead, not just the attempt.
    let code = extract_otp_code(&aframp::otp::mock::last_message_for(&normalized_phone).unwrap());
    let (status, body) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge_id, "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "OTP_LOCKED");
}

#[tokio::test]
async fn otp_expired_challenge_rejected() {
    let Some((app, db)) = app_and_db().await else {
        return;
    };
    let (email, phone_number, normalized_phone) = fresh_identity("expired");
    let (_, challenge) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": email, "password": "password123", "name": "Expired", "phone_number": phone_number })),
    )
    .await;
    let challenge_id = challenge["challenge_id"].as_str().unwrap();

    sqlx::query("UPDATE otp_challenges SET expires_at = now() - interval '1 minute' WHERE id = $1::uuid")
        .bind(challenge_id)
        .execute(&db)
        .await
        .unwrap();

    let code = extract_otp_code(&aframp::otp::mock::last_message_for(&normalized_phone).unwrap());
    let (status, body) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge_id, "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "OTP_EXPIRED");
}

#[tokio::test]
async fn otp_resend_cooldown_429_within_60s() {
    let Some(app) = app().await else {
        return;
    };
    let (email, phone_number, _) = fresh_identity("cooldown");
    let body = json!({ "email": email, "password": "password123", "name": "Cooldown", "phone_number": phone_number });

    let (status1, _) = send(app.clone(), "POST", "/signup", None, Some(body.clone())).await;
    assert_eq!(status1, StatusCode::OK);

    let (status2, body2) = send(app.clone(), "POST", "/signup", None, Some(body)).await;
    assert_eq!(status2, StatusCode::TOO_MANY_REQUESTS, "{body2}");
    assert_eq!(body2["code"], "TOO_MANY_REQUESTS");
}

#[tokio::test]
async fn otp_resend_after_cooldown_uses_new_code() {
    let Some((app, db)) = app_and_db().await else {
        return;
    };
    let (email, phone_number, normalized_phone) = fresh_identity("refresh");
    let body = json!({ "email": email, "password": "password123", "name": "Refresh", "phone_number": phone_number });

    let (status1, challenge1) = send(app.clone(), "POST", "/signup", None, Some(body.clone())).await;
    assert_eq!(status1, StatusCode::OK);
    let old_code = extract_otp_code(&aframp::otp::mock::last_message_for(&normalized_phone).unwrap());

    // Fast-forward past the cooldown on the existing challenge.
    sqlx::query("UPDATE otp_challenges SET last_sent_at = now() - interval '61 seconds' WHERE id = $1::uuid")
        .bind(challenge1["challenge_id"].as_str().unwrap())
        .execute(&db)
        .await
        .unwrap();

    let (status2, challenge2) = send(app.clone(), "POST", "/signup", None, Some(body)).await;
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(
        challenge1["challenge_id"], challenge2["challenge_id"],
        "a resend refreshes the same challenge row, it doesn't create a new one"
    );
    let new_code = extract_otp_code(&aframp::otp::mock::last_message_for(&normalized_phone).unwrap());

    // The old code is dead; only the freshly-sent one verifies.
    let (status, body) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge2["challenge_id"], "code": old_code })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge2["challenge_id"], "code": new_code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn otp_rate_limit_429_after_five_in_an_hour() {
    let Some((app, db)) = app_and_db().await else {
        return;
    };
    let (email, phone_number, normalized_phone) = fresh_identity("spam");

    // Seed 5 already-expired challenges for this phone directly — each still
    // counts toward the hourly cap (it's keyed on created_at), but none of
    // them are "live", so the next signup can't just refresh one in place.
    // Keyed on the normalized (E.164) form, same as the app stores it.
    for _ in 0..5 {
        sqlx::query(
            "INSERT INTO otp_challenges
                 (purpose, pending_email, pending_password_hash, pending_name, phone_number, code_hash, expires_at)
             VALUES ('signup', $1, 'x', 'x', $2, 'x', now() - interval '1 minute')",
        )
        .bind(&email)
        .bind(&normalized_phone)
        .execute(&db)
        .await
        .unwrap();
    }

    let (status, body) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": email, "password": "password123", "name": "Spam", "phone_number": phone_number })),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["code"], "TOO_MANY_REQUESTS");
}

#[tokio::test]
async fn me_returns_profile_for_a_valid_token() {
    let Some(app) = app().await else {
        return;
    };
    let (email, _, verified) = signup_and_verify(&app, "me").await;
    let token = verified["token"].as_str().unwrap();

    let (status, me) = send(app.clone(), "GET", "/me", Some(token), None).await;
    assert_eq!(status, StatusCode::OK, "me failed: {me}");
    assert_eq!(me["email"], email);
    assert_eq!(me["name"], "Test User");
    assert_eq!(me["user_id"], verified["user_id"]);
    assert_eq!(me["merchant_id"], verified["merchant_id"]);
    assert_eq!(me["merchant_name"], "Test User");
    assert!(
        me.get("password_hash").is_none(),
        "the password hash must never be serialized to a client"
    );
}

#[tokio::test]
async fn login_sets_an_http_only_session_cookie_that_authenticates() {
    let Some(app) = app().await else {
        return;
    };
    let (email, normalized_phone, _) = signup_and_verify(&app, "cookie").await;

    let (status, challenge) = send(
        app.clone(),
        "POST",
        "/login",
        None,
        Some(json!({ "email": email, "password": "password123" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login failed: {challenge}");
    let code = extract_otp_code(&aframp::otp::mock::last_message_for(&normalized_phone).unwrap());

    let (status, verified, cookies) = send_with_cookie(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge["challenge_id"], "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "verify-otp failed: {verified}");

    let session = cookies
        .iter()
        .find(|c| c.starts_with("aframp_session="))
        .expect("verify-otp must set a session cookie");
    assert!(session.contains("HttpOnly"), "session must be unreadable from JS: {session}");
    assert!(session.contains("Secure"), "session must not travel over plain HTTP: {session}");
    assert!(session.contains("SameSite=Lax"), "session must not ride cross-site requests: {session}");

    // The cookie alone authenticates: no Authorization header in sight.
    let jar = format!("aframp_session={}", verified["token"].as_str().unwrap());
    let (status, me, _) = send_with_cookie(app.clone(), "GET", "/me", Some(&jar), None).await;
    assert_eq!(status, StatusCode::OK, "cookie auth failed: {me}");
    assert_eq!(me["email"], email);
}

#[tokio::test]
async fn logout_clears_the_session_cookie() {
    let Some(app) = app().await else {
        return;
    };
    let (status, _, cookies) =
        send_with_cookie(app.clone(), "POST", "/logout", None, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let cleared = cookies
        .iter()
        .find(|c| c.starts_with("aframp_session="))
        .expect("logout must clear the session cookie");
    assert!(cleared.contains("Max-Age=0"), "cookie must expire immediately: {cleared}");
}

#[tokio::test]
async fn a_garbage_session_cookie_is_rejected() {
    let Some(app) = app().await else {
        return;
    };
    let (status, _, _) = send_with_cookie(
        app.clone(),
        "GET",
        "/me",
        Some("aframp_session=not-a-real-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_requires_a_valid_token() {
    let Some(app) = app().await else {
        return;
    };
    let (status, _) = send(app.clone(), "GET", "/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(app.clone(), "GET", "/me", Some("not-a-real-token"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
