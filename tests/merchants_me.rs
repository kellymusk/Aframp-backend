mod common;

use axum::http::StatusCode;

use common::{ensure_merchant, send, state};

#[tokio::test]
async fn merchants_me_returns_the_callers_merchant() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "merchants_me").await;

    let (status, body) = send(app.clone(), "GET", "/merchants/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "merchants/me failed: {body}");
    assert_eq!(body["id"], merchant_id);
    assert!(body["name"].is_string());
}

#[tokio::test]
async fn merchants_me_requires_auth() {
    let state = state().await;
    let app = aframp::router(state);
    let (status, _) = send(app, "GET", "/merchants/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
