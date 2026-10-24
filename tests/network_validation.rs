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
