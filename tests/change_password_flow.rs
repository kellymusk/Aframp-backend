//! #1254 — `POST /auth/change-password`.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

use common::{extract_otp_code, send, state};

async fn app() -> axum::Router {
    aframp::router(state().await)
}

/// Signs up and verifies a fresh account, returning `(email, session token)`.
async fn signup_and_verify(app: &axum::Router, seed: &str) -> (String, String) {
    let unique = Uuid::new_v4();
    let email = format!("{seed}+{}@example.com", unique.simple());
    let local_digits = (unique.as_u128() as u64) % 10_000_000_000;
    let phone_number = format!("0{local_digits:010}");
    let normalized_phone = format!("+234{local_digits:010}");

    let (status, challenge) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(json!({ "email": email, "password": "password123", "name": "Pw User", "phone_number": phone_number })),
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
    (email, verified["token"].as_str().unwrap().to_string())
}

/// `POST /login` answers 200 (an OTP challenge) for a correct password and
/// 401 for a wrong one, so its status tells us which password is current.
async fn login_status(app: &axum::Router, email: &str, password: &str) -> StatusCode {
    send(app.clone(), "POST", "/login", None, Some(json!({ "email": email, "password": password })))
        .await
        .0
}

#[tokio::test]
async fn change_password_replaces_the_password_and_revokes_older_sessions() {
    let app = app().await;
    let (email, old_token) = signup_and_verify(&app, "pwchange").await;
    // Session validity has one-second (JWT `iat`) resolution; make sure the
    // existing session clearly predates the change.
    tokio::time::sleep(Duration::from_millis(1100)).await;

    let (status, body) = send(
        app.clone(),
        "POST",
        "/auth/change-password",
        Some(&old_token),
        Some(json!({ "current_password": "password123", "new_password": "brand-new-pass" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "change-password failed: {body}");
    let new_token = body["token"].as_str().expect("a fresh session token").to_string();
    assert_ne!(new_token, old_token);

    // The session that made the change, and any other one, is revoked...
    let (status, _) = send(app.clone(), "GET", "/me", Some(&old_token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "pre-change session must be revoked");
    let (status, _) = send(app.clone(), "POST", "/auth/refresh", Some(&old_token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a revoked session must not be refreshable");

    // ...while the replacement session works.
    let (status, me) = send(app.clone(), "GET", "/me", Some(&new_token), None).await;
    assert_eq!(status, StatusCode::OK, "replacement session must work: {me}");
    assert_eq!(me["email"], email);

    // Only the new password logs in now.
    assert_eq!(login_status(&app, &email, "password123").await, StatusCode::UNAUTHORIZED);
    assert_eq!(login_status(&app, &email, "brand-new-pass").await, StatusCode::OK);
}

#[tokio::test]
async fn change_password_rejects_a_wrong_current_password() {
    let app = app().await;
    let (email, token) = signup_and_verify(&app, "pwwrong").await;

    let (status, body) = send(
        app.clone(),
        "POST",
        "/auth/change-password",
        Some(&token),
        Some(json!({ "current_password": "not-my-password", "new_password": "brand-new-pass" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["field"], "current_password");

    // Nothing changed: the session still works and the old password still logs in.
    let (status, _) = send(app.clone(), "GET", "/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(login_status(&app, &email, "password123").await, StatusCode::OK);
    assert_eq!(login_status(&app, &email, "brand-new-pass").await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn change_password_enforces_the_minimum_length() {
    let app = app().await;
    let (email, token) = signup_and_verify(&app, "pwshort").await;

    let (status, body) = send(
        app.clone(),
        "POST",
        "/auth/change-password",
        Some(&token),
        Some(json!({ "current_password": "password123", "new_password": "short" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["field"], "new_password");
    assert_eq!(login_status(&app, &email, "password123").await, StatusCode::OK);
}

#[tokio::test]
async fn change_password_requires_a_session() {
    let app = app().await;
    let (status, _) = send(
        app.clone(),
        "POST",
        "/auth/change-password",
        None,
        Some(json!({ "current_password": "password123", "new_password": "brand-new-pass" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
