//! #1243 — `PATCH /merchants/me` and `PATCH /users/me`.
mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, state};

#[tokio::test]
async fn merchant_can_rename_business() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "merchant_rename").await;

    let (status, json) = send(
        app.clone(),
        "PATCH",
        "/merchants/me",
        Some(&token),
        Some(json!({ "name": "  Ada's Bakery  " })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "rename failed: {json}");
    assert_eq!(json["merchant_id"], merchant_id);
    assert_eq!(json["name"], "Ada's Bakery", "name should be trimmed by validate_name");

    let (_, me) = send(app.clone(), "GET", "/me", Some(&token), None).await;
    assert_eq!(me["merchant_name"], "Ada's Bakery");
}

#[tokio::test]
async fn merchant_update_rejects_empty_body_and_invalid_name() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "merchant_rename_bad").await;

    let (status, _) = send(app.clone(), "PATCH", "/merchants/me", Some(&token), Some(json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(app.clone(), "PATCH", "/merchants/me", Some(&token), Some(json!({ "name": "   " }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) =
        send(app.clone(), "PATCH", "/merchants/me", Some(&token), Some(json!({ "email": "not-an-email" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn merchant_email_change_requires_confirmation() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "merchant_email").await;

    let (status, json) = send(
        app.clone(),
        "PATCH",
        "/merchants/me",
        Some(&token),
        Some(json!({ "email": "New@Example.com" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "email change request failed: {json}");
    assert_eq!(json["pending_email"], "new@example.com");
    assert_ne!(json["email"], "new@example.com", "email must not change before confirmation");

    let (status, _) = send(
        app.clone(),
        "POST",
        "/merchants/me/email/confirm",
        Some(&token),
        Some(json!({ "token": "wrong" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a wrong token must be rejected");

    // Plant a known token so the confirmation path can be exercised without
    // an outbound email transport.
    let token_value = "e2e-known-token";
    let hash = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(token_value.as_bytes()))
    };
    sqlx::query("UPDATE merchants SET email_change_token_hash = $2 WHERE id = $1::uuid")
        .bind(&merchant_id)
        .bind(hash)
        .execute(&state.db)
        .await
        .unwrap();

    let (status, json) = send(
        app.clone(),
        "POST",
        "/merchants/me/email/confirm",
        Some(&token),
        Some(json!({ "token": token_value })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "confirm failed: {json}");
    assert_eq!(json["email"], "new@example.com");
    assert!(json["pending_email"].is_null());
}

#[tokio::test]
async fn user_can_rename_via_users_me() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "user_rename").await;

    let (status, json) =
        send(app.clone(), "PATCH", "/users/me", Some(&token), Some(json!({ "name": "Grace Hopper" }))).await;
    assert_eq!(status, StatusCode::OK, "rename failed: {json}");
    assert_eq!(json["name"], "Grace Hopper");
}
