mod common;

use std::sync::Arc;

use aframp::payments::mock::MockProvider;
use aframp::payments::{PaymentProvider, PayoutRequest, PayoutResult};
use async_trait::async_trait;
use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, state};

struct FailingProvider;

#[async_trait]
impl PaymentProvider for FailingProvider {
    async fn create_payout(&self, _req: &PayoutRequest) -> Result<PayoutResult, String> {
        Err("simulated provider failure".into())
    }
}

/// Simulates the Paystack error returned when the destination bank code
/// isn't a recognized institution code.
struct InvalidBankCodeProvider;

#[async_trait]
impl PaymentProvider for InvalidBankCodeProvider {
    async fn create_payout(&self, _req: &PayoutRequest) -> Result<PayoutResult, String> {
        Err("Invalid bank code".into())
    }
}

/// Simulates the Paystack error returned when the account number doesn't
/// resolve for the given bank.
struct InvalidAccountNumberProvider;

#[async_trait]
impl PaymentProvider for InvalidAccountNumberProvider {
    async fn create_payout(&self, _req: &PayoutRequest) -> Result<PayoutResult, String> {
        Err("Could not resolve account number".into())
    }
}

/// Simulates the request to Paystack timing out before a response is
/// received.
struct TimeoutProvider;

#[async_trait]
impl PaymentProvider for TimeoutProvider {
    async fn create_payout(&self, _req: &PayoutRequest) -> Result<PayoutResult, String> {
        Err("request to Paystack timed out".into())
    }
}

#[tokio::test]
async fn withdrawal_insufficient_balance_rejected() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "insufficient").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 1_000_000,
            "asset": "cNGN",
            "bank_code": "058",
            "account_number": "0123456789"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "expected rejection: {json}");
    assert_eq!(json["error"], "insufficient available balance");
}

#[tokio::test]
async fn withdrawal_validates_bank_details() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "validation").await;

    let (status, _) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 1_000_000,
            "bank_code": "",
            "account_number": "123"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn withdrawal_success_decrements_balance() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "withdraw_ok").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 5_000_000, 0)
         ON CONFLICT (merchant_id, asset) DO UPDATE SET available = 5_000_000, pending = 0",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let (status, json) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 2_000_000,
            "asset": "cNGN",
            "bank_code": "058",
            "account_number": "0123456789"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "withdraw failed: {json}");
    assert_eq!(json["status"], "pending");
    assert_eq!(json["amount_stroops"], 2_000_000);

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 3_000_000, "available balance should be debited");

    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn withdrawal_full_balance_then_insufficient() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "drain").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 1_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let (status, _) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 1_000_000,
            "bank_code": "058",
            "account_number": "0123456789"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 1,
            "bank_code": "058",
            "account_number": "0123456789"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "second withdrawal should fail");
}

#[tokio::test]
async fn withdrawal_unsupported_asset_rejected() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "unsupported_asset").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'XLM', 5_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let (status, json) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 1_000_000,
            "asset": "XLM",
            "bank_code": "058",
            "account_number": "0123456789"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "expected rejection: {json}");
    assert_eq!(json["error"], "withdrawals are only supported for the cNGN asset");
}

#[tokio::test]
async fn withdrawal_rejects_sub_kobo_precision() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "precision").await;

    // Balance is large enough that insufficient-balance can't be the reason
    // this is rejected — isolates the precision check specifically.
    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 10_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let (status, json) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 1_000_050, // not a multiple of 100,000
            "asset": "cNGN",
            "bank_code": "058",
            "account_number": "0123456789"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "expected rejection: {json}");
    assert_eq!(json["error"], "amount_stroops must be a whole number of kobo");
}

#[tokio::test]
async fn withdrawal_payout_failure_refunds_balance_and_records_reason() {
    let Some(mut state) = state().await else {
        return;
    };
    // Swap in a provider that always fails, to exercise the compensating
    // refund + audit-trail path without needing a real Paystack failure.
    state.payment_provider = Arc::new(FailingProvider);
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "payout_fail").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 5_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let (status, json) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 2_000_000,
            "asset": "cNGN",
            "bank_code": "058",
            "account_number": "0123456789"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "expected payout failure: {json}");
    assert_eq!(json["error"], "simulated provider failure");

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 5_000_000, "balance should be refunded after a failed payout");

    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let withdrawals = json.as_array().unwrap();
    assert_eq!(withdrawals.len(), 1, "the failed attempt should still leave an audit-trail row");
    assert_eq!(withdrawals[0]["status"], "failed");
    assert_eq!(withdrawals[0]["failure_reason"], "simulated provider failure");
}

#[tokio::test]
async fn withdrawal_insufficient_balance_never_calls_provider() {
    let Some(mut state) = state().await else {
        return;
    };
    // A MockProvider always succeeds, so if this withdrawal were rejected
    // for any reason other than the balance check, this test would see a
    // 200 instead of the expected 400 — this isolates the balance check as
    // happening before the provider is ever invoked.
    state.payment_provider = Arc::new(MockProvider);
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "insufficient_mock").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 1_000_000,
            "asset": "cNGN",
            "bank_code": "058",
            "account_number": "0123456789"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "expected rejection: {json}");
    assert_eq!(json["error"], "insufficient available balance");
}

#[tokio::test]
async fn withdrawal_invalid_bank_code_refunds_balance_and_records_reason() {
    let Some(mut state) = state().await else {
        return;
    };
    state.payment_provider = Arc::new(InvalidBankCodeProvider);
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "invalid_bank_code").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 5_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let (status, json) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 2_000_000,
            "asset": "cNGN",
            "bank_code": "999",
            "account_number": "0123456789"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "expected payout failure: {json}");
    assert_eq!(json["error"], "Invalid bank code");

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 5_000_000, "balance should be refunded after an invalid bank code");

    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let withdrawals = json.as_array().unwrap();
    assert_eq!(withdrawals[0]["status"], "failed");
    assert_eq!(withdrawals[0]["failure_reason"], "Invalid bank code");
}

#[tokio::test]
async fn withdrawal_invalid_account_number_refunds_balance_and_records_reason() {
    let Some(mut state) = state().await else {
        return;
    };
    state.payment_provider = Arc::new(InvalidAccountNumberProvider);
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "invalid_account_number").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 5_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let (status, json) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 2_000_000,
            "asset": "cNGN",
            "bank_code": "058",
            "account_number": "0000000000"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "expected payout failure: {json}");
    assert_eq!(json["error"], "Could not resolve account number");

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 5_000_000, "balance should be refunded after an invalid account number");

    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let withdrawals = json.as_array().unwrap();
    assert_eq!(withdrawals[0]["status"], "failed");
    assert_eq!(withdrawals[0]["failure_reason"], "Could not resolve account number");
}

#[tokio::test]
async fn withdrawal_paystack_timeout_refunds_balance_and_records_reason() {
    let Some(mut state) = state().await else {
        return;
    };
    state.payment_provider = Arc::new(TimeoutProvider);
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "paystack_timeout").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 5_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let (status, json) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 2_000_000,
            "asset": "cNGN",
            "bank_code": "058",
            "account_number": "0123456789"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "expected payout failure: {json}");
    assert_eq!(json["error"], "request to Paystack timed out");

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 5_000_000, "balance should be refunded after a provider timeout");

    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let withdrawals = json.as_array().unwrap();
    assert_eq!(withdrawals[0]["status"], "failed");
    assert_eq!(withdrawals[0]["failure_reason"], "request to Paystack timed out");
}
