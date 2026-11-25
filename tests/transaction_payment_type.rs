/// Integration test for issue #1464.
///
/// Verifies that `GET /transactions` exposes `payment_type` on every row and
/// that the value correctly distinguishes a customer deposit (`"deposit"`) from
/// an internal settlement sweep (`"sweep"`).
mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, state};

async fn app() -> axum::Router {
    aframp::router(state().await)
}

/// Create one deposit row and one sweep row for the same merchant, then assert
/// that `GET /transactions` returns both with the correct `payment_type` value.
#[tokio::test]
async fn deposit_and_sweep_have_correct_payment_type() {
    let state = state().await;
    let app = aframp::router(state.clone());

    let (token, merchant_id) = ensure_merchant(&app, "tx_payment_type").await;

    // Create a wallet so we have a valid wallet_id / wallet_address.
    let (status, wallet) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "wallet create failed: {wallet}");
    let wallet_id = wallet["id"].as_str().unwrap();
    let wallet_address = wallet["address"].as_str().unwrap();

    // Insert a deposit row (payment_type = 'deposit').
    let deposit_hash = format!("tx_deposit_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO payments
           (merchant_id, wallet_id, wallet_address, tx_hash,
            amount_stroops, asset, network, status, payment_type)
         VALUES ($1::uuid, $2::uuid, $3, $4, 50000000, 'XLM', 'stellar', 'confirmed', 'deposit')",
    )
    .bind(&merchant_id)
    .bind(wallet_id)
    .bind(wallet_address)
    .bind(&deposit_hash)
    .execute(&state.db)
    .await
    .expect("inserting deposit row failed");

    // Insert a sweep row (payment_type = 'sweep'), mirroring what
    // services::payments::record_sweep() does.
    let sweep_hash = format!("tx_sweep_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO payments
           (merchant_id, wallet_id, wallet_address, tx_hash,
            amount_stroops, asset, network, status, payment_type)
         VALUES ($1::uuid, $2::uuid, $3, $4, 50000000, 'XLM', 'stellar', 'confirmed', 'sweep')",
    )
    .bind(&merchant_id)
    .bind(wallet_id)
    .bind(wallet_address)
    .bind(&sweep_hash)
    .execute(&state.db)
    .await
    .expect("inserting sweep row failed");

    // Fetch the transaction list for this merchant.
    let (status, body) = send(app.clone(), "GET", "/transactions", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "GET /transactions failed: {body}");

    let rows = body["data"].as_array().expect("data should be an array");

    // Both rows must be present.
    assert!(rows.len() >= 2, "expected at least 2 rows, got {}: {body}", rows.len());

    // Find the deposit and sweep rows by tx_hash.
    let deposit_row = rows
        .iter()
        .find(|r| r["tx_hash"].as_str() == Some(&deposit_hash))
        .unwrap_or_else(|| panic!("deposit row not found in response: {body}"));

    let sweep_row = rows
        .iter()
        .find(|r| r["tx_hash"].as_str() == Some(&sweep_hash))
        .unwrap_or_else(|| panic!("sweep row not found in response: {body}"));

    // payment_type must be present and have the correct value.
    assert_eq!(
        deposit_row["payment_type"].as_str(),
        Some("deposit"),
        "deposit row has wrong payment_type: {deposit_row}"
    );
    assert_eq!(
        sweep_row["payment_type"].as_str(),
        Some("sweep"),
        "sweep row has wrong payment_type: {sweep_row}"
    );
}

/// Rows inserted without an explicit payment_type (e.g. legacy rows that
/// pre-date the column) must still appear in `GET /transactions` — they should
/// not cause a deserialisation error, and `payment_type` should be either
/// `"deposit"` (column default) or `null` (nullable variant) depending on the
/// migration applied.
///
/// This test only asserts the row is present and the endpoint returns 200; the
/// exact value of `payment_type` for legacy rows is an implementation detail
/// left to the migration author.
#[tokio::test]
async fn legacy_row_without_payment_type_still_appears() {
    let state = state().await;
    let app = aframp::router(state.clone());

    let (token, merchant_id) = ensure_merchant(&app, "tx_legacy_payment_type").await;

    let (status, wallet) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "wallet create failed: {wallet}");
    let wallet_id = wallet["id"].as_str().unwrap();
    let wallet_address = wallet["address"].as_str().unwrap();

    // Insert a row using only the columns that existed before #1464, letting
    // the database apply any default for payment_type.
    let legacy_hash = format!("tx_legacy_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO payments
           (merchant_id, wallet_id, wallet_address, tx_hash,
            amount_stroops, asset, network, status)
         VALUES ($1::uuid, $2::uuid, $3, $4, 10000000, 'XLM', 'stellar', 'confirmed')",
    )
    .bind(&merchant_id)
    .bind(wallet_id)
    .bind(wallet_address)
    .bind(&legacy_hash)
    .execute(&state.db)
    .await
    .expect("inserting legacy row failed");

    let (status, body) = send(app.clone(), "GET", "/transactions", Some(&token), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /transactions failed for merchant with legacy row: {body}"
    );

    let rows = body["data"].as_array().expect("data should be an array");
    let legacy_row = rows
        .iter()
        .find(|r| r["tx_hash"].as_str() == Some(&legacy_hash))
        .unwrap_or_else(|| panic!("legacy row not found in response: {body}"));

    // payment_type must be present in the JSON object (may be null or "deposit").
    assert!(
        legacy_row.as_object().map_or(false, |o| o.contains_key("payment_type")),
        "payment_type key missing from legacy row: {legacy_row}"
    );
}
