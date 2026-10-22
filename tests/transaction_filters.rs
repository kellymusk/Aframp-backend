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

    let (status, body) = send(
        app.clone(),
        "GET",
        "/transactions?status=pending",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_PARAMETERS");

    let (status, body) = send(
        app.clone(),
        "GET",
        "/transactions?asset=",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_PARAMETERS");
}

#[tokio::test]
async fn transactions_filters_narrow_the_result_set() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "tx_filters_rows").await;
    let (status, wallet) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "wallet create failed: {wallet}");

    for (asset, status) in [
        ("XLM", "confirmed"),
        ("XLM", "detected"),
        ("cNGN", "confirmed"),
    ] {
        sqlx::query(
            "INSERT INTO payments
               (merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status)
             VALUES ($1::uuid, $2::uuid, $3, $4, 1000, $5, 'stellar', $6)",
        )
        .bind(&merchant_id)
        .bind(wallet["id"].as_str().unwrap())
        .bind(wallet["address"].as_str().unwrap())
        .bind(format!("tx_filter_{}", uuid::Uuid::new_v4().simple()))
        .bind(asset)
        .bind(status)
        .execute(&state.db)
        .await
        .unwrap();
    }

    for (path, expected) in [
        ("/transactions", 3),
        ("/transactions?asset=XLM", 2),
        ("/transactions?status=confirmed", 2),
        ("/transactions?asset=XLM&status=detected", 1),
    ] {
        let (status, body) = send(app.clone(), "GET", path, Some(&token), None).await;
        assert_eq!(status, StatusCode::OK, "{path} failed: {body}");
        assert_eq!(
            body["data"].as_array().unwrap().len(),
            expected,
            "{path}: {body}"
        );
    }
}
