//! #1003 — merchant API keys: create (session only), authenticate, list, revoke.
mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, state};

#[tokio::test]
async fn api_key_lifecycle() {
    let app = aframp::router(state().await);
    let (token, merchant_id) = ensure_merchant(&app, "api_keys").await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/api-keys",
        Some(&token),
        Some(json!({ "environment": "test" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let key = created["secret"].as_str().unwrap().to_string();
    assert!(key.starts_with("sk_test_"));
    let key_id = created["id"].as_str().unwrap().to_string();

    // The key authenticates as the merchant.
    let (status, me) = send(app.clone(), "GET", "/me", Some(&key), None).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["merchant_id"], merchant_id);

    // A key can't mint another key.
    let (status, _) = send(app.clone(), "POST", "/api-keys", Some(&key), Some(json!({}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Listed without the secret.
    let (status, list) = send(app.clone(), "GET", "/api-keys", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let keys = list.as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert!(keys[0].get("secret").is_none() && keys[0].get("secret_hash").is_none());

    // Revoked keys stop working.
    let (status, _) = send(app.clone(), "DELETE", &format!("/api-keys/{key_id}"), Some(&token), None).await;
    assert!(status.is_success(), "revoke failed: {status}");
    let (status, _) = send(app.clone(), "GET", "/me", Some(&key), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn malformed_or_unknown_api_key_is_rejected() {
    let app = aframp::router(state().await);
    let unknown = format!("sk_test_{}", "a".repeat(40));
    let (status, _) = send(app.clone(), "GET", "/me", Some(&unknown), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
