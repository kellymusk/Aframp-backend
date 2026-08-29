mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, state};

async fn app() -> Option<axum::Router> {
    state().await.map(aframp::router)
}

#[tokio::test]
async fn protected_routes_require_token() {
    let Some(app) = app().await else {
        return;
    };
    for (method, path) in [
        ("POST", "/wallet/create"),
        ("GET", "/wallet"),
        ("GET", "/balance"),
        ("GET", "/transactions"),
        ("POST", "/withdraw"),
        ("GET", "/withdrawals"),
    ] {
        let (status, json) = send(app.clone(), method, path, None, None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "expected 401 for {method} {path}: {json}"
        );
    }
}

#[tokio::test]
async fn create_and_fetch_wallet() {
    let Some(app) = app().await else {
        return;
    };
    let (token, _) = ensure_merchant(&app, "wallet").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create wallet failed: {json}");
    let address = json["address"].as_str().unwrap().to_string();
    assert!(!address.is_empty());
    assert_eq!(json["network"], "stellar");

    let (status, json) = send(app.clone(), "GET", "/wallet", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "get wallet failed: {json}");
    assert_eq!(json["address"], address);
}

#[tokio::test]
async fn balance_and_transactions_start_empty() {
    let Some(app) = app().await else {
        return;
    };
    let (token, _) = ensure_merchant(&app, "empty").await;

    let (status, json) = send(app.clone(), "GET", "/balance", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "balance failed: {json}");
    assert_eq!(json, json!([]));

    let (status, json) = send(app.clone(), "GET", "/transactions", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "transactions failed: {json}");
    assert_eq!(json, json!([]));
}

#[tokio::test]
async fn wallet_address_is_stable_per_merchant() {
    let Some(app) = app().await else {
        return;
    };
    let (token_a, _) = ensure_merchant(&app, "stable_a").await;
    let (token_b, _) = ensure_merchant(&app, "stable_b").await;

    send(app.clone(), "POST", "/wallet/create", Some(&token_a), Some(json!({}))).await;
    send(app.clone(), "POST", "/wallet/create", Some(&token_b), Some(json!({}))).await;

    let (_, json_a) = send(app.clone(), "GET", "/wallet", Some(&token_a), None).await;
    let (_, json_b) = send(app.clone(), "GET", "/wallet", Some(&token_b), None).await;
    assert_ne!(json_a["address"], json_b["address"]);
}
