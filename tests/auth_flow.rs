mod common;

use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

use common::{extract_otp_code, send, send_with_cookie, state};

async fn app() -> axum::Router {
    aframp::router(state().await)
}

/// Like `app()`, but also hands back a raw `PgPool` for tests that need to
/// force an OTP challenge into a particular state (expired, stale) directly.
async fn app_and_db() -> (axum::Router, sqlx::PgPool) {
    let state = common::state().await;
    let db = state.db.clone();
    (aframp::router(state), db)
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
    let app = app().await;
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
    assert!(
        challenge.get("token").is_none(),
        "signup must not issue a session before OTP verification"
    );
    let challenge_id = challenge["challenge_id"].as_str().unwrap();
    assert!(challenge["expires_in_secs"].as_i64().unwrap() > 0);

    let message = aframp::otp::mock::last_message_for(&normalized_phone)
        .expect("mock provider should record the message");
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
    let app = app().await;
    let (email, phone_number, _) = fresh_identity("pending");

    let body = json!({ "email": email, "password": "password123", "name": "Pending", "phone_number": phone_number });
    let (status1, first) = send(app.clone(), "POST", "/signup", None, Some(body.clone())).await;
    assert_eq!(status1, StatusCode::OK, "first signup failed: {first}");

    // Neither challenge has been verified — resubmitting is a resend, not a conflict.
    let (status2, second) = send(app.clone(), "POST", "/signup", None, Some(body)).await;
    assert_eq!(
        status2,
        StatusCode::TOO_MANY_REQUESTS,
        "immediate resend should hit the cooldown, not a conflict: {second}"
    );
}

#[tokio::test]
async fn signup_duplicate_email_conflicts_after_verification() {
    let app = app().await;
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
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "expected conflict: {conflict}"
    );
    assert_eq!(conflict["code"], "EMAIL_TAKEN");
}

#[tokio::test]
async fn signup_weak_password_rejected() {
    let app = app().await;
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
    let app = app().await;
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
    let app = app().await;
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
    assert!(
        challenge.get("token").is_none(),
        "login for a phone-verified account must not issue a session directly"
    );
    let challenge_id = challenge["challenge_id"]
        .as_str()
        .expect("login should return a fresh challenge_id");

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
    let (app, db) = app_and_db().await;
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
    assert_eq!(
        status,
        StatusCode::OK,
        "legacy login should succeed synchronously: {body}"
    );
    assert!(
        body["token"].as_str().unwrap().len() > 10,
        "legacy login must issue a session directly, no OTP"
    );
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
    let app = app().await;
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
    let app = app().await;
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
    let app = app().await;
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
    let app = app().await;
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
    let (app, db) = app_and_db().await;
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

    sqlx::query(
        "UPDATE otp_challenges SET expires_at = now() - interval '1 minute' WHERE id = $1::uuid",
    )
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
    let app = app().await;
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
    let (app, db) = app_and_db().await;
    let (email, phone_number, normalized_phone) = fresh_identity("refresh");
    let body = json!({ "email": email, "password": "password123", "name": "Refresh", "phone_number": phone_number });

    let (status1, challenge1) =
        send(app.clone(), "POST", "/signup", None, Some(body.clone())).await;
    assert_eq!(status1, StatusCode::OK);
    let old_code =
        extract_otp_code(&aframp::otp::mock::last_message_for(&normalized_phone).unwrap());

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
    let new_code =
        extract_otp_code(&aframp::otp::mock::last_message_for(&normalized_phone).unwrap());

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
    let (app, db) = app_and_db().await;
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
    let app = app().await;
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
    let app = app().await;
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
    assert!(
        session.contains("HttpOnly"),
        "session must be unreadable from JS: {session}"
    );
    assert!(
        session.contains("Secure"),
        "session must not travel over plain HTTP: {session}"
    );
    assert!(
        session.contains("SameSite=Lax"),
        "session must not ride cross-site requests: {session}"
    );

    // The cookie alone authenticates: no Authorization header in sight.
    let jar = format!("aframp_session={}", verified["token"].as_str().unwrap());
    let (status, me, _) = send_with_cookie(app.clone(), "GET", "/me", Some(&jar), None).await;
    assert_eq!(status, StatusCode::OK, "cookie auth failed: {me}");
    assert_eq!(me["email"], email);
}

#[tokio::test]
async fn logout_clears_the_session_cookie() {
    let app = app().await;
    let (status, _, cookies) =
        send_with_cookie(app.clone(), "POST", "/logout", None, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let cleared = cookies
        .iter()
        .find(|c| c.starts_with("aframp_session="))
        .expect("logout must clear the session cookie");
    assert!(
        cleared.contains("Max-Age=0"),
        "cookie must expire immediately: {cleared}"
    );
}

#[tokio::test]
async fn a_garbage_session_cookie_is_rejected() {
    let app = app().await;
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
    let app = app().await;
    let (status, _) = send(app.clone(), "GET", "/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(app.clone(), "GET", "/me", Some("not-a-real-token"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_users_list_never_includes_password_hash() {
    let (app, db) = app_and_db().await;
    let email = format!("admin+{}@example.com", Uuid::new_v4().simple());
    let password = "legacy-password-123";
    let hash = aframp_password_hash_for_tests();
    sqlx::query("INSERT INTO users (email, password_hash, name, is_admin) VALUES ($1, $2, 'Admin User', true)")
        .bind(&email)
        .bind(&hash)
        .execute(&db)
        .await
        .unwrap();

    let (status, login) = send(
        app.clone(),
        "POST",
        "/login",
        None,
        Some(json!({ "email": email, "password": password })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "admin login failed: {login}");
    let token = login["token"]
        .as_str()
        .expect("legacy admin login should return a token");

    let (status, users) = send(app.clone(), "GET", "/admin/users", Some(token), None).await;
    assert_eq!(status, StatusCode::OK, "admin users failed: {users}");
    let rows = users
        .as_array()
        .expect("/admin/users should return an array");
    let current_admin = rows
        .iter()
        .find(|row| row["email"].as_str() == Some(email.as_str()))
        .expect("newly-created admin should be visible in /admin/users");
    assert!(
        current_admin.get("password_hash").is_none(),
        "/admin/users must never leak password hashes"
    );
}

#[tokio::test]
async fn signup_oversized_name_rejected_before_work() {
    let app = app().await;
    let (email, phone_number, _) = fresh_identity("longname");
    let name = "N".repeat(101);
    let (status, body) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": email, "password": "password123", "name": name, "phone_number": phone_number })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["field"], "name");
}

#[tokio::test]
async fn signup_oversized_email_local_part_rejected() {
    let app = app().await;
    let (_, phone_number, _) = fresh_identity("longemail");
    // Local-part longer than 64 is rejected by is_valid_email (and by RFC 5321).
    let email = format!("{}@example.com", "a".repeat(65));
    let (status, body) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": email, "password": "password123", "name": "Long Email", "phone_number": phone_number })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["field"], "email");
}

#[tokio::test]
async fn signup_oversized_phone_rejected() {
    let app = app().await;
    let (email, _, _) = fresh_identity("longphone");
    let (status, body) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({
            "email": email,
            "password": "password123",
            "name": "Long Phone",
            "phone_number": format!("080{}", "1".repeat(40))
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["field"], "phone_number");
}

#[tokio::test]
async fn verified_signup_challenge_is_deleted() {
    let (app, db) = app_and_db().await;
    let (email, phone_number, normalized_phone) = fresh_identity("challenge_gone");

    let (status, challenge) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": email, "password": "password123", "name": "Gone", "phone_number": phone_number })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "signup failed: {challenge}");
    let challenge_id: uuid::Uuid = challenge["challenge_id"].as_str().unwrap().parse().unwrap();
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

    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM otp_challenges WHERE id = $1")
        .bind(challenge_id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(remaining, 0, "the challenge (and its pending password hash) must be gone");

    // The same code can't be replayed once the row is gone.
    let (status, _) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge_id, "code": code })),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
async fn stale_challenges_are_purged_when_a_new_one_is_issued() {
    let (app, db) = app_and_db().await;
    let (email, phone_number, _) = fresh_identity("stale_purge");
    let (status, challenge) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": email, "password": "password123", "name": "Stale", "phone_number": phone_number })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "signup failed: {challenge}");
    let stale_id: uuid::Uuid = challenge["challenge_id"].as_str().unwrap().parse().unwrap();

    // Age the abandoned challenge past the 24h retention window.
    sqlx::query(
        "UPDATE otp_challenges
            SET created_at = now() - interval '25 hours', expires_at = now() - interval '25 hours'
          WHERE id = $1",
    )
    .bind(stale_id)
    .execute(&db)
    .await
    .unwrap();

    let (other_email, other_phone, _) = fresh_identity("stale_purge_trigger");
    let (status, _) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": other_email, "password": "password123", "name": "Trigger", "phone_number": other_phone })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM otp_challenges WHERE id = $1")
        .bind(stale_id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(remaining, 0, "a challenge expired over 24h ago must be hard-deleted");
}

#[tokio::test]
async fn revoking_admin_takes_effect_on_the_next_request() {
    let (app, db) = app_and_db().await;
    // A legacy (no-phone) account logs in without OTP, which keeps this test
    // focused on the admin check.
    let email = format!("admin+{}@example.com", Uuid::new_v4().simple());
    sqlx::query("INSERT INTO users (email, password_hash, name, is_admin) VALUES ($1, $2, 'Admin', true)")
        .bind(&email)
        .bind(aframp_password_hash_for_tests())
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
    assert_eq!(status, StatusCode::OK, "login failed: {body}");
    let token = body["token"].as_str().unwrap().to_string();

    let (status, body) = send(app.clone(), "GET", "/admin/overview", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "admin should have access: {body}");

    sqlx::query("UPDATE users SET is_admin = false WHERE email = $1")
        .bind(&email)
        .execute(&db)
        .await
        .unwrap();

    // Same, still-unexpired token: rejected straight away.
    let (status, _) = send(app.clone(), "GET", "/admin/overview", Some(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Signs a token with the integration-test secret, bypassing the app, so
/// tests can present tokens the server would never issue itself.
fn forge_token(sub: Uuid, iat: i64, exp: i64, orig_iat: i64) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &json!({
            "sub": sub,
            "merchant_id": null,
            "is_admin": false,
            "iat": iat,
            "exp": exp,
            "orig_iat": orig_iat,
        }),
        &jsonwebtoken::EncodingKey::from_secret(b"integration-test-secret"),
    )
    .unwrap()
}

#[tokio::test]
async fn refresh_issues_a_working_token() {
    let state = common::state().await;
    let app = aframp::router(state);
    let (token, merchant_id) = common::ensure_merchant(&app, "refresh_ok").await;

    let (status, body) = send(app.clone(), "POST", "/auth/refresh", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "refresh failed: {body}");
    assert_eq!(body["merchant_id"], merchant_id);
    let refreshed = body["token"].as_str().unwrap();

    let (status, me) = send(app.clone(), "GET", "/me", Some(refreshed), None).await;
    assert_eq!(status, StatusCode::OK, "refreshed token rejected: {me}");
}

#[tokio::test]
async fn refresh_after_logout_has_no_session_to_refresh() {
    let app = app().await;
    let (token, _) = common::ensure_merchant(&app, "refresh_logout").await;
    let cookie = format!("aframp_session={token}");

    let (status, _, set_cookie) = send_with_cookie(app.clone(), "POST", "/logout", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(set_cookie.iter().any(|c| c.starts_with("aframp_session=;")), "logout clears the cookie");

    // The browser now holds the cleared cookie, so there's nothing to refresh.
    let (status, _, _) = send_with_cookie(app.clone(), "POST", "/auth/refresh", Some("aframp_session="), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn refresh_rejects_an_expired_token() {
    let app = app().await;
    let now = chrono::Utc::now().timestamp();
    let expired = forge_token(Uuid::new_v4(), now - 2 * 86_400, now - 86_400, now - 2 * 86_400);

    let (status, _) = send(app.clone(), "POST", "/auth/refresh", Some(&expired), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn refresh_stops_after_the_seven_day_session_window() {
    let app = app().await;
    // Forge the token for a real, active account: tokens for unknown or
    // deleted users are rejected before the session window is checked.
    let (_, _, verified) = signup_and_verify(&app, "refresh_window").await;
    let sub: Uuid = verified["user_id"].as_str().unwrap().parse().unwrap();
    let now = chrono::Utc::now().timestamp();
    // Still unexpired, but the session started eight days ago.
    let stale = forge_token(sub, now - 3_600, now + 3_600, now - 8 * 86_400);

    let (status, body) = send(app.clone(), "POST", "/auth/refresh", Some(&stale), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["error"].as_str().unwrap().contains("log in again"), "{body}");
}
