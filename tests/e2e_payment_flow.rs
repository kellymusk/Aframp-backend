//! #1245 — Canonical "it all works" end-to-end smoke test.
//!
//! Walks a brand-new merchant through the whole happy path in one test,
//! asserting each step before moving on:
//!   1. Signup → OTP verify (mock OTP provider) → session token
//!   2. Create wallet → Stellar G-address
//!   3. Create payment request → SEP-7 URI carrying the memo
//!   4. Simulate deposit detection (insert a `payments` row, apply the
//!      balance delta, correlate by memo) → request goes to `paid`
//!   5. `GET /balance` reflects the deposit
//!
//! If this test is green, the core merchant flow works end to end. Step 4
//! mirrors `blockchain::worker::process_deposit`, driving the same service
//! calls directly so no live Stellar network is needed.
mod common;

use aframp::models::{NewPayment, UpdateBalance, UpdatePaymentStatus};
use aframp::services::{balances, payment_requests, payments, wallets};
use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

use common::{ensure_merchant, send, state};

const AMOUNT_STROOPS: i64 = 42_000_000;

#[tokio::test]
async fn signup_wallet_payment_request_deposit_balance_end_to_end() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());

    // 1. Signup → OTP verify → token (mock OTP provider delivers the code).
    let (token, merchant_id_str) = ensure_merchant(&app, "e2e_flow").await;
    assert!(!token.is_empty(), "verify-otp must issue a session token");
    let merchant_id: Uuid = merchant_id_str.parse().expect("merchant_id should be a uuid");

    let (status, me) = send(app.clone(), "GET", "/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "token should authenticate /me: {me}");
    assert_eq!(me["merchant_id"], merchant_id_str);

    // 2. Create wallet → G-address.
    let (status, wallet_json) = send(app.clone(), "POST", "/wallet/create", Some(&token), Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "wallet create failed: {wallet_json}");
    let address = wallet_json["address"].as_str().expect("wallet should have an address").to_string();
    assert!(address.starts_with('G') && address.len() == 56, "expected a Stellar G-address, got {address}");
    let wallet = wallets::wallet_by_merchant(&state.db, merchant_id)
        .await
        .unwrap()
        .expect("wallet row should exist after create");
    assert_eq!(wallet.address, address);

    // 3. Create payment request → SEP-7 URI.
    let (status, pr) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": AMOUNT_STROOPS })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "payment request create failed: {pr}");
    assert_eq!(pr["status"], "pending");
    let pr_id = pr["id"].as_str().unwrap().to_string();
    let memo = pr["memo"].as_str().unwrap().to_string();
    let sep7 = pr["sep7_uri"].as_str().expect("XLM request should carry a sep7_uri");
    assert!(sep7.starts_with("web+stellar:pay?destination="), "{sep7}");
    assert!(sep7.contains(&address), "SEP-7 URI should pay the merchant wallet: {sep7}");
    assert!(sep7.contains(&format!("memo={memo}")), "SEP-7 URI should carry the memo: {sep7}");

    // 4. Simulate deposit detection exactly as the deposit worker does.
    let payment = payments::record_deposit(
        &state.db,
        NewPayment {
            merchant_id,
            wallet_id: wallet.id,
            wallet_address: address.clone(),
            tx_hash: format!("tx_e2e_{}", Uuid::new_v4().simple()),
            amount_stroops: AMOUNT_STROOPS,
            asset: "XLM".into(),
            network: "stellar".into(),
        },
    )
    .await
    .expect("record_deposit should succeed");
    payments::set_status(&state.db, payment.id, UpdatePaymentStatus::Confirmed)
        .await
        .expect("set_status Confirmed should succeed");
    balances::apply_delta(
        &state.db,
        &UpdateBalance {
            merchant_id,
            asset: "XLM".into(),
            available_delta: AMOUNT_STROOPS,
            pending_delta: 0,
        },
    )
    .await
    .expect("apply_delta should succeed");
    let pending = payment_requests::find_pending_by_wallet_and_memo(&state.db, wallet.id, &memo)
        .await
        .expect("memo lookup should not error")
        .expect("the deposit memo should match the pending request");
    assert_eq!(pending.id.to_string(), pr_id);
    payment_requests::mark_paid(&state.db, pending.id, payment.id)
        .await
        .expect("mark_paid should succeed");

    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{pr_id}"), None, None).await;
    assert_eq!(status, StatusCode::OK, "public fetch failed: {fetched}");
    assert_eq!(fetched["status"], "paid", "request should be paid after the deposit: {fetched}");

    // 5. Balance reflects the deposit.
    let (status, balance) = send(app.clone(), "GET", "/balance", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "balance failed: {balance}");
    let rows = balance.as_array().expect("GET /balance returns a JSON array");
    assert_eq!(rows.len(), 1, "one deposit asset → one balance row: {balance}");
    assert_eq!(rows[0]["asset"], "XLM");
    assert_eq!(rows[0]["available"], AMOUNT_STROOPS);
}
