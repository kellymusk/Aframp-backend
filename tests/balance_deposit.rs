//! #1228 — `GET /balance` must reflect the real detected/confirmed deposits
//! that flowed through the deposit-worker logic, not just return 200.
//!
//! `seed_confirmed_deposit` mimics the final step of
//! `blockchain::worker::process_deposit`: a confirmed `payments` row plus the
//! matching `balances::apply_delta` credit.

mod common;

use aframp::models::UpdateBalance;
use aframp::services::balances;
use axum::http::StatusCode;
use common::{ensure_merchant, send, state};

/// Insert the `payments` row a confirmed deposit would have produced and apply
/// the confirmed balance delta through `balances::apply_delta`.
async fn seed_confirmed_deposit(
    state: &aframp::AppState,
    merchant_id: &str,
    asset: &str,
    amount_stroops: i64,
) {
    let address = format!("G{}", uuid::Uuid::new_v4().simple().to_string().to_uppercase());
    let wallet_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO wallets (merchant_id, address, network)
         VALUES ($1::uuid, $2, 'stellar')
         RETURNING id",
    )
    .bind(merchant_id)
    .bind(&address)
    .fetch_one(&state.db)
    .await
    .expect("seed wallet");

    sqlx::query(
        "INSERT INTO payments
           (merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status)
         VALUES ($1::uuid, $2, $3, $4, $5, $6, 'stellar', 'confirmed')",
    )
    .bind(merchant_id)
    .bind(wallet_id)
    .bind(&address)
    .bind(format!("tx_{}", uuid::Uuid::new_v4().simple()))
    .bind(amount_stroops)
    .bind(asset)
    .execute(&state.db)
    .await
    .expect("seed confirmed payment");

    balances::apply_delta(
        &state.db,
        &UpdateBalance::confirmed_deposit(
            uuid::Uuid::parse_str(merchant_id).unwrap(),
            asset,
            amount_stroops,
        ),
    )
    .await
    .expect("apply confirmed deposit delta");
}

#[tokio::test]
async fn balance_reflects_a_confirmed_deposit() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "bal_confirmed").await;
    let amount = 42_000_000_i64; // 4.2 XLM in stroops

    seed_confirmed_deposit(&state, &merchant_id, "XLM", amount).await;

    let (status, json) = send(app.clone(), "GET", "/balance", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "balance failed: {json}");
    let rows = json.as_array().expect("GET /balance returns a JSON array");
    assert_eq!(rows.len(), 1, "one deposited asset → one row: {json}");
    assert_eq!(rows[0]["asset"], "XLM");
    assert_eq!(
        rows[0]["available"].as_i64(),
        Some(amount),
        "available must equal the confirmed deposit: {json}"
    );
    assert_eq!(rows[0]["pending"].as_i64(), Some(0));
}

#[tokio::test]
async fn balance_reports_each_asset_separately() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "bal_multi_asset").await;

    seed_confirmed_deposit(&state, &merchant_id, "XLM", 10_000_000).await;
    seed_confirmed_deposit(&state, &merchant_id, "cNGN", 25_000).await;

    let (status, json) = send(app.clone(), "GET", "/balance", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "balance failed: {json}");
    let rows = json.as_array().expect("GET /balance returns a JSON array");
    assert_eq!(rows.len(), 2, "two assets → two rows: {json}");

    let find = |asset: &str| {
        rows.iter()
            .find(|r| r["asset"] == asset)
            .unwrap_or_else(|| panic!("missing {asset} row in {json}"))
    };
    assert_eq!(find("XLM")["available"].as_i64(), Some(10_000_000));
    assert_eq!(find("cNGN")["available"].as_i64(), Some(25_000));
}

#[tokio::test]
async fn balance_is_empty_without_deposits() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state);
    let (token, _) = ensure_merchant(&app, "bal_zero").await;

    let (status, json) = send(app.clone(), "GET", "/balance", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "balance failed: {json}");
    assert_eq!(json, serde_json::json!([]), "no deposits → empty array");
}
