mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, state};

async fn app() -> axum::Router {
    aframp::router(state().await)
}

#[tokio::test]
async fn transactions_accept_asset_and_status_filters() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "tx_filters").await;

    for path in [
        "/transactions?asset=XLM",
        "/transactions?status=confirmed",
        "/transactions?asset=cNGN&status=detected&limit=10",
    ] {
        let (status, body) = send(app.clone(), "GET", path, Some(&token), None).await;
        assert_eq!(status, StatusCode::OK, "{path} failed: {body}");
        assert_eq!(body["data"], json!([]), "{path} should be empty: {body}");
        assert!(body["next_cursor"].is_null(), "{path}: {body}");
    }
}

#[tokio::test]
async fn transactions_reject_unknown_status() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "tx_filters_bad").await;

    let (status, body) = send(app.clone(), "GET", "/transactions?status=pending", Some(&token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_PARAMETERS");

    let (status, body) = send(app.clone(), "GET", "/transactions?asset=", Some(&token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_PARAMETERS");
}
