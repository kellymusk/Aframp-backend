//! #1126 — GET /transactions, /withdrawals, and /payment-requests limit
//! boundary matrix: clamp 0→1 and 201→200, honour 1/200, default 50, and
//! reject non-numeric `limit` without a 500.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use uuid::Uuid;

use common::{ensure_merchant, send, state};

/// List endpoints return either a bare array (legacy) or a cursor page
/// `{ "data": [...], "next_cursor": ... }`. Prefer `data` when present.
fn page_items(json: &Value) -> &[Value] {
    if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
        return data.as_slice();
    }
    json.as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

fn assert_bad_limit(status: StatusCode, json: &Value) {
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "non-numeric limit must be 400 (not 500): status={status} body={json}"
    );
    // Documented API error format (API.md): { "error": "...", "code": "..." }.
    // When the body is JSON, require that contract so regressions can't sneak
    // a plain-text or 500-shaped failure back in.
    if json.is_object() {
        assert!(
            json.get("error").and_then(|e| e.as_str()).is_some(),
            "API error must include human `error` string: {json}"
        );
        let code = json.get("code").and_then(|c| c.as_str());
        assert_eq!(
            code,
            Some("INVALID_PARAMETERS"),
            "bad limit must use INVALID_PARAMETERS: {json}"
        );
    } else {
        // Axum's default Query rejection is plain text today. Still not a 500,
        // which is the regression this issue guards against. See
        // docs/proposals/1126-query-limit-error-shape.md for the follow-up
        // that maps the rejection onto the documented JSON error shape.
        assert!(
            json.is_null() || json.is_string(),
            "unexpected body for limit=abc: {json}"
        );
    }
}

async fn ensure_wallet(app: &axum::Router, token: &str) -> String {
    let (status, json) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "wallet create failed: {json}");
    json["address"].as_str().unwrap().to_string()
}

async fn seed_payments(db: &sqlx::PgPool, merchant_id: &str, address: &str, n: i64) {
    let wallet_id: Uuid = sqlx::query_scalar("SELECT id FROM wallets WHERE address = $1")
        .bind(address)
        .fetch_one(db)
        .await
        .expect("wallet row");
    let merchant: Uuid = merchant_id.parse().unwrap();
    for i in 0..n {
        sqlx::query(
            "INSERT INTO payments
               (merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status)
             VALUES ($1, $2, $3, $4, 1000, 'XLM', 'stellar', 'confirmed')",
        )
        .bind(merchant)
        .bind(wallet_id)
        .bind(address)
        .bind(format!("lim_pay_{merchant_id}_{i}"))
        .execute(db)
        .await
        .unwrap_or_else(|e| panic!("seed payment {i}: {e}"));
    }
}

async fn seed_withdrawals(db: &sqlx::PgPool, merchant_id: &str, n: i64) {
    let merchant: Uuid = merchant_id.parse().unwrap();
    for i in 0..n {
        sqlx::query(
            "INSERT INTO withdrawals
               (merchant_id, amount_stroops, asset, status, bank_code, account_number)
             VALUES ($1, 100000, 'cNGN', 'failed', '058', '0123456789')",
        )
        .bind(merchant)
        .execute(db)
        .await
        .unwrap_or_else(|e| panic!("seed withdrawal {i}: {e}"));
    }
}

async fn seed_payment_requests(db: &sqlx::PgPool, merchant_id: &str, address: &str, n: i64) {
    let wallet_id: Uuid = sqlx::query_scalar("SELECT id FROM wallets WHERE address = $1")
        .bind(address)
        .fetch_one(db)
        .await
        .expect("wallet row");
    let merchant: Uuid = merchant_id.parse().unwrap();
    for i in 0..n {
        // memo is globally UNIQUE — include merchant id + index.
        let memo = format!("{}{i:04}", &merchant_id[..8]);
        sqlx::query(
            "INSERT INTO payment_requests
               (merchant_id, wallet_id, amount_stroops, asset, memo, status, expires_at)
             VALUES ($1, $2, 1000000, 'XLM', $3, 'pending', now() + interval '1 hour')",
        )
        .bind(merchant)
        .bind(wallet_id)
        .bind(&memo)
        .execute(db)
        .await
        .unwrap_or_else(|e| panic!("seed payment request {i}: {e}"));
    }
}

async fn assert_limit_matrix(
    app: axum::Router,
    token: &str,
    path: &str,
) {
    // Missing limit → default 50
    let (status, json) = send(app.clone(), path, Some(token), None).await;
    assert_eq!(status, StatusCode::OK, "default limit on {path}: {json}");
    assert_eq!(
        page_items(&json).len(),
        50,
        "{path}: missing ?limit must default to 50"
    );

    // limit=0 → clamped to 1
    let (status, json) = send(app.clone(), &format!("{path}?limit=0"), Some(token), None).await;
    assert_eq!(status, StatusCode::OK, "{path}?limit=0: {json}");
    assert_eq!(
        page_items(&json).len(),
        1,
        "{path}: limit=0 must clamp to 1, not return 0 rows or error"
    );

    // limit=1
    let (status, json) = send(app.clone(), &format!("{path}?limit=1"), Some(token), None).await;
    assert_eq!(status, StatusCode::OK, "{path}?limit=1: {json}");
    assert_eq!(page_items(&json).len(), 1);

    // limit=200
    let (status, json) = send(app.clone(), &format!("{path}?limit=200"), Some(token), None).await;
    assert_eq!(status, StatusCode::OK, "{path}?limit=200: {json}");
    assert_eq!(page_items(&json).len(), 200);

    // limit=201 → clamped to 200
    let (status, json) = send(app.clone(), &format!("{path}?limit=201"), Some(token), None).await;
    assert_eq!(status, StatusCode::OK, "{path}?limit=201: {json}");
    assert_eq!(
        page_items(&json).len(),
        200,
        "{path}: limit=201 must clamp to 200"
    );

    // limit=abc → 400, never 500
    let (status, json) = send(app.clone(), &format!("{path}?limit=abc"), Some(token), None).await;
    assert_bad_limit(status, &json);
}

#[tokio::test]
async fn transactions_limit_boundaries() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "lim_tx").await;
    let address = ensure_wallet(&app, &token).await;
    seed_payments(&state.db, &merchant_id, &address, 210).await;
    assert_limit_matrix(app, &token, "/transactions").await;
}

#[tokio::test]
async fn withdrawals_limit_boundaries() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "lim_wd").await;
    let _address = ensure_wallet(&app, &token).await;
    seed_withdrawals(&state.db, &merchant_id, 210).await;
    assert_limit_matrix(app, &token, "/withdrawals").await;
}

#[tokio::test]
async fn payment_requests_limit_boundaries() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "lim_pr").await;
    let address = ensure_wallet(&app, &token).await;
    seed_payment_requests(&state.db, &merchant_id, &address, 210).await;
    assert_limit_matrix(app, &token, "/payment-requests").await;
}
