//! #1125 — GET /balance for merchants with no deposits, after a deposit,
//! and while a confirmation is still pending.

mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, state};

#[tokio::test]
async fn new_merchant_with_no_deposits_gets_empty_balance_array() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state);
    let (token, _) = ensure_merchant(&app, "bal_empty").await;

    let (status, json) = send(app.clone(), "GET", "/balance", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "expected 200, got {status}: {json}");
    assert_eq!(
        json,
        json!([]),
        "brand-new merchant must get [] — not 404, not an error object"
    );
}

#[tokio::test]
async fn after_one_xlm_deposit_balance_returns_one_entry_with_correct_amounts() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "bal_deposit").await;
    let amount = 25_000_000_i64; // 2.5 XLM in stroops

    // Confirmed deposit: available credited, pending cleared (mirrors the
    // final step of blockchain::worker::process_deposit).
    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'XLM', $2, 0)
         ON CONFLICT (merchant_id, asset)
         DO UPDATE SET available = $2, pending = 0, updated_at = now()",
    )
    .bind(&merchant_id)
    .bind(amount)
    .execute(&state.db)
    .await
    .expect("seed confirmed XLM balance");

    let (status, json) = send(app.clone(), "GET", "/balance", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "balance failed: {json}");
    let rows = json.as_array().expect("GET /balance returns a JSON array");
    assert_eq!(rows.len(), 1, "one deposit asset → one balance row: {json}");
    assert_eq!(rows[0]["asset"], "XLM");
    assert_eq!(rows[0]["available"], amount);
    assert_eq!(rows[0]["pending"], 0);
    assert_eq!(rows[0]["merchant_id"], merchant_id);
}

#[tokio::test]
async fn pending_deposit_shows_pending_gt_zero_and_available_zero() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "bal_pending").await;
    let amount = 10_000_000_i64;

    // Detected but not yet confirmed — the intermediate ledger state before
    // the confirmation-depth threshold moves pending → available.
    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'XLM', 0, $2)
         ON CONFLICT (merchant_id, asset)
         DO UPDATE SET available = 0, pending = $2, updated_at = now()",
    )
    .bind(&merchant_id)
    .bind(amount)
    .execute(&state.db)
    .await
    .expect("seed pending XLM balance");

    let (status, json) = send(app.clone(), "GET", "/balance", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "balance failed: {json}");
    let rows = json.as_array().expect("GET /balance returns a JSON array");
    assert_eq!(rows.len(), 1, "{json}");
    assert_eq!(rows[0]["asset"], "XLM");
    assert_eq!(rows[0]["available"], 0, "still unconfirmed → available must be 0");
    assert!(
        rows[0]["pending"].as_i64().unwrap() > 0,
        "pending must be > 0 while confirmation is outstanding: {json}"
    );
    assert_eq!(rows[0]["pending"], amount);
}
