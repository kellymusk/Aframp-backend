mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, state};

async fn app() -> axum::Router {
    aframp::router(state().await)
}

#[tokio::test]
async fn rejects_unknown_network_with_field_error() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "netvalidate-unknown").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({ "network": "ethereum" })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["field"], "network");
}

#[tokio::test]
async fn rejects_empty_network_with_field_error() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "netvalidate-empty").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({ "network": "" })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["field"], "network");
}

#[tokio::test]
async fn accepts_stellar_network() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "netvalidate-stellar").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({ "network": "stellar" })),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{json}");
}

#[tokio::test]
async fn wallet_create_accepts_an_empty_body() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "netvalidate-empty-body").await;
    let (status, json) = send(app.clone(), "POST", "/wallet/create", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["network"], "stellar");
}

#[tokio::test]
async fn wallet_create_rejects_malformed_json() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "netvalidate-bad-json").await;
    let request = axum::http::Request::post("/wallet/create")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{not json"))
        .unwrap();
    let response = tower::ServiceExt::oneshot(app, request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
