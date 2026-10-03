/// #1040 — Tests for the deposit-worker payment-request correlation logic.
///
/// The worker's `process_deposit` is an internal function, so these tests drive
/// the same service-layer calls it makes:
///   1. `record_deposit` → inserts the payment row (status = "detected")
///   2. `set_status` (Verified → Confirmed) → advances the status
///   3. `balances::apply_delta` → credits the merchant's balance
///   4. `find_pending_by_wallet_and_memo` → looks up the matching request
///   5. `mark_paid` / `mark_partial` → correlates deposit to request
///
/// This mirrors exactly what `blockchain::worker::process_deposit` does and
/// was established as the correct pattern by `payment_request_marked_paid_on_memo_correlated_deposit`
/// in `payment_request_flow.rs`.
mod common;

use aframp::models::{NewPayment, UpdateBalance, UpdatePaymentStatus};
use aframp::services::{balances, payment_requests, payments, wallets};
use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

use common::{ensure_merchant, send, state};

/// Simulate exactly what `blockchain::worker::process_deposit` does for a
/// single deposit — record it, advance status, apply balance delta, and run
/// the memo-correlation branch.
async fn simulate_deposit(
    state: &aframp::AppState,
    wallet_id: Uuid,
    merchant_id: Uuid,
    wallet_address: &str,
    tx_hash: &str,
    amount_stroops: i64,
    memo: Option<&str>,
) {
    let payment = payments::record_deposit(
        &state.db,
        NewPayment {
            merchant_id,
            wallet_id,
            wallet_address: wallet_address.to_string(),
            tx_hash: tx_hash.to_string(),
            amount_stroops,
            asset: "XLM".into(),
            network: "stellar".into(),
        },
    )
    .await
    .expect("record_deposit should succeed");

    assert_eq!(payment.status, "detected");

    payments::set_status(&state.db, payment.id, UpdatePaymentStatus::Verified)
        .await
        .expect("set_status Verified should succeed");

    balances::apply_delta(
        &state.db,
        &UpdateBalance {
            merchant_id,
            asset: "XLM".into(),
            available_delta: 0,
            pending_delta: amount_stroops,
        },
    )
    .await
    .expect("apply_delta pending should succeed");

    payments::set_status(&state.db, payment.id, UpdatePaymentStatus::Confirmed)
        .await
        .expect("set_status Confirmed should succeed");

    balances::apply_delta(
        &state.db,
        &UpdateBalance {
            merchant_id,
            asset: "XLM".into(),
            available_delta: amount_stroops,
            pending_delta: -amount_stroops,
        },
    )
    .await
    .expect("apply_delta available should succeed");

    // Memo correlation — mirrors the final if-block in process_deposit.
    if let Some(memo) = memo {
        if let Some(pr) =
            payment_requests::find_pending_by_wallet_and_memo(&state.db, wallet_id, memo)
                .await
                .expect("find_pending_by_wallet_and_memo should not error")
        {
            if payment.amount_stroops >= pr.amount_stroops {
                payment_requests::mark_paid(&state.db, pr.id, payment.id)
                    .await
                    .expect("mark_paid should succeed");
            } else {
                payment_requests::mark_partial(&state.db, pr.id, payment.id)
                    .await
                    .expect("mark_partial should succeed");
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// #1040 test 1: deposit with matching memo and correct amount marks request paid
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn deposit_with_matching_memo_and_exact_amount_marks_request_paid() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id_str) = ensure_merchant(&app, "corr_paid").await;
    let merchant_id: Uuid = merchant_id_str.parse().unwrap();

    // Create wallet via HTTP so the row is in DB.
    let (status, wallet_json) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "wallet create: {wallet_json}");
    let wallet = wallets::wallet_by_merchant(&state.db, merchant_id)
        .await
        .unwrap()
        .expect("wallet should exist after create");

    // Create a payment request.
    let (status, pr_json) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 25_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create payment request: {pr_json}");
    let pr_id = pr_json["id"].as_str().unwrap();
    let memo = pr_json["memo"].as_str().unwrap().to_string();

    // Simulate a deposit whose amount exactly equals the request.
    simulate_deposit(
        &state,
        wallet.id,
        merchant_id,
        &wallet.address,
        &format!("tx_exact_{}", Uuid::new_v4().simple()),
        25_000_000,
        Some(&memo),
    )
    .await;

    // The payment request should now be "paid".
    let (status, fetched) =
        send(app.clone(), "GET", &format!("/payment-requests/{pr_id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        fetched["status"], "paid",
        "exact-amount deposit with matching memo should mark request paid: {fetched}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// #1040 test 2: deposit with matching memo but smaller amount marks request partial
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn deposit_with_matching_memo_but_smaller_amount_marks_request_partial() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id_str) = ensure_merchant(&app, "corr_partial").await;
    let merchant_id: Uuid = merchant_id_str.parse().unwrap();

    let (status, _) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let wallet = wallets::wallet_by_merchant(&state.db, merchant_id)
        .await
        .unwrap()
        .expect("wallet should exist");

    let (status, pr_json) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 25_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create payment request: {pr_json}");
    let pr_id = pr_json["id"].as_str().unwrap();
    let memo = pr_json["memo"].as_str().unwrap().to_string();

    // Deposit less than the requested amount.
    simulate_deposit(
        &state,
        wallet.id,
        merchant_id,
        &wallet.address,
        &format!("tx_partial_{}", Uuid::new_v4().simple()),
        10_000_000, // underpaid
        Some(&memo),
    )
    .await;

    let (status, fetched) =
        send(app.clone(), "GET", &format!("/payment-requests/{pr_id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        fetched["status"], "partial",
        "underpaid deposit with matching memo should mark request partial: {fetched}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// #1040 test 3: deposit with non-matching memo does not affect any payment request
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn deposit_with_non_matching_memo_does_not_affect_payment_request() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id_str) = ensure_merchant(&app, "corr_nomatch").await;
    let merchant_id: Uuid = merchant_id_str.parse().unwrap();

    let (status, _) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let wallet = wallets::wallet_by_merchant(&state.db, merchant_id)
        .await
        .unwrap()
        .expect("wallet should exist");

    let (status, pr_json) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 25_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create payment request: {pr_json}");
    let pr_id = pr_json["id"].as_str().unwrap();

    // Deposit with a completely different memo.
    simulate_deposit(
        &state,
        wallet.id,
        merchant_id,
        &wallet.address,
        &format!("tx_wrong_memo_{}", Uuid::new_v4().simple()),
        25_000_000,
        Some("deadbeefdeadbeef"), // wrong memo
    )
    .await;

    let (status, fetched) =
        send(app.clone(), "GET", &format!("/payment-requests/{pr_id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        fetched["status"], "pending",
        "non-matching memo should leave the payment request pending: {fetched}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// #1040 test 4: deposit with no memo does not affect any payment request
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn deposit_with_no_memo_does_not_affect_payment_request() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id_str) = ensure_merchant(&app, "corr_nomemo").await;
    let merchant_id: Uuid = merchant_id_str.parse().unwrap();

    let (status, _) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let wallet = wallets::wallet_by_merchant(&state.db, merchant_id)
        .await
        .unwrap()
        .expect("wallet should exist");

    let (status, pr_json) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 25_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create payment request: {pr_json}");
    let pr_id = pr_json["id"].as_str().unwrap();

    // Deposit with no memo at all.
    simulate_deposit(
        &state,
        wallet.id,
        merchant_id,
        &wallet.address,
        &format!("tx_no_memo_{}", Uuid::new_v4().simple()),
        25_000_000,
        None, // no memo
    )
    .await;

    let (status, fetched) =
        send(app.clone(), "GET", &format!("/payment-requests/{pr_id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        fetched["status"], "pending",
        "memo-less deposit should leave the payment request pending: {fetched}"
    );
}
