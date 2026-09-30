mod common;

use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

use common::{ensure_merchant, extract_otp_code, send, state};

/// A fresh, collision-free (local, E.164) Nigerian phone number pair.
fn fresh_phone() -> (String, String) {
    let digits = (Uuid::new_v4().as_u128() as u64) % 10_000_000_000;
    (format!("0{digits:010}"), format!("+234{digits:010}"))
}

#[tokio::test]
async fn patch_me_updates_the_name() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state);
    let (token, _) = ensure_merchant(&app, "profile_name").await;

    let (status, body) = send(app.clone(), "PATCH", "/me", Some(&token), Some(json!({ "name": "  Ada Obi  " }))).await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");
    assert_eq!(body["name"], "Ada Obi");
    assert!(body["phone_verification"].is_null());

    let (_, me) = send(app.clone(), "GET", "/me", Some(&token), None).await;
    assert_eq!(me["name"], "Ada Obi");
}

#[tokio::test]
async fn patch_me_changes_the_phone_only_after_otp_verification() {
    let Some(state) = state().await else {
        return;
    };
    let db = state.db.clone();
    let app = aframp::router(state);
    let (token, _) = ensure_merchant(&app, "profile_phone").await;
    let (local, e164) = fresh_phone();

    let (status, body) = send(app.clone(), "PATCH", "/me", Some(&token), Some(json!({ "phone_number": local }))).await;
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");
    let challenge_id = body["phone_verification"]["challenge_id"].as_str().expect("a challenge is issued").to_string();
    assert_ne!(body["phone_number"], e164, "the number must not change before verification");

    let code = extract_otp_code(&aframp::otp::mock::last_message_for(&e164).expect("code sent to the new number"));
    let (status, verified) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge_id, "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "verify failed: {verified}");

    let user_id: Uuid = verified["user_id"].as_str().unwrap().parse().unwrap();
    let phone: Option<String> = sqlx::query_scalar("SELECT phone_number FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(phone.as_deref(), Some(e164.as_str()));
}

#[tokio::test]
async fn patch_me_rejects_invalid_input() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state);
    let (token, _) = ensure_merchant(&app, "profile_invalid").await;

    let (status, _) = send(app.clone(), "PATCH", "/me", Some(&token), Some(json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "an empty update is rejected");

    let (status, body) = send(app.clone(), "PATCH", "/me", Some(&token), Some(json!({ "name": "   " }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["field"], "name");

    let (status, body) = send(app.clone(), "PATCH", "/me", Some(&token), Some(json!({ "phone_number": "12345" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["field"], "phone_number");

    let (status, _) = send(app.clone(), "PATCH", "/me", None, Some(json!({ "name": "Nobody" }))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn patch_me_refuses_a_phone_owned_by_another_account() {
    let Some(state) = state().await else {
        return;
    };
    let db = state.db.clone();
    let app = aframp::router(state);
    let (token, _) = ensure_merchant(&app, "profile_taken").await;
    let (other_token, _) = ensure_merchant(&app, "profile_owner").await;
    let (_, other_me) = send(app.clone(), "GET", "/me", Some(&other_token), None).await;
    let other_id: Uuid = other_me["user_id"].as_str().unwrap().parse().unwrap();
    let taken: String = sqlx::query_scalar("SELECT phone_number FROM users WHERE id = $1")
        .bind(other_id)
        .fetch_one(&db)
        .await
        .unwrap();

    let (status, _) = send(app.clone(), "PATCH", "/me", Some(&token), Some(json!({ "phone_number": taken }))).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn delete_me_anonymizes_the_account_and_revokes_its_tokens() {
    let Some(state) = state().await else {
        return;
    };
    let db = state.db.clone();
    let app = aframp::router(state);
    let (token, merchant_id) = ensure_merchant(&app, "profile_delete").await;
    let (_, me) = send(app.clone(), "GET", "/me", Some(&token), None).await;
    let user_id: Uuid = me["user_id"].as_str().unwrap().parse().unwrap();
    let email = me["email"].as_str().unwrap().to_string();

    // Financial records must survive the deletion.
    let (status, wallet) = send(app.clone(), "POST", "/wallet/create", Some(&token), Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "wallet create failed: {wallet}");
    let (status, request) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 10_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "payment request failed: {request}");

    let (status, _) = send(app.clone(), "DELETE", "/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Every outstanding token stops working, including for refresh.
    let (status, _) = send(app.clone(), "GET", "/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(app.clone(), "POST", "/auth/refresh", Some(&token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // PII is anonymized in place.
    let (stored_email, name, phone, deleted): (String, String, Option<String>, bool) = sqlx::query_as(
        "SELECT email, name, phone_number, deleted_at IS NOT NULL FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_ne!(stored_email, email);
    assert!(stored_email.ends_with("@deleted.invalid"));
    assert_eq!(name, "Deleted user");
    assert!(phone.is_none());
    assert!(deleted);

    // The old credentials can't log back in.
    let (status, _) = send(
        app.clone(),
        "POST",
        "/login",
        None,
        Some(json!({ "email": email, "password": "password123" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The payment request (financial audit trail) is still there.
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_requests WHERE merchant_id = $1::uuid")
        .bind(&merchant_id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(remaining, 1);
}

#[tokio::test]
async fn delete_me_requires_authentication() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state);
    let (status, _) = send(app.clone(), "DELETE", "/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
