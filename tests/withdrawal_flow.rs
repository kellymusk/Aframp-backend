mod common;

use std::sync::Arc;

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
async fn withdrawal_daily_limit_exceeded_rejected() {
    let Some(mut state) = state().await else {
        return;
    };
    state.daily_withdrawal_limit_stroops = Some(3_000_000);
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "daily_limit_exceeded").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 10_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    // Completed withdrawal earlier today of 2,000,000 stroops
    sqlx::query(
        "INSERT INTO withdrawals (merchant_id, amount_stroops, asset, status, bank_code, account_number, created_at, updated_at)
         VALUES ($1::uuid, 2_000_000, 'cNGN', 'completed', '058', '0123456789', now(), now())",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    // Attempting 2,000,000 more (total 4,000,000 > 3,000,000 limit)
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
    assert_eq!(status, StatusCode::BAD_REQUEST, "expected daily limit rejection: {json}");
    assert_eq!(json["error"], "daily withdrawal limit exceeded");

    // Balance should remain unchanged
    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 10_000_000);
}

#[tokio::test]
async fn withdrawal_daily_limit_within_limit_allowed() {
    let Some(mut state) = state().await else {
        return;
    };
    state.daily_withdrawal_limit_stroops = Some(5_000_000);
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "daily_limit_within").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 10_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    // Completed withdrawal earlier today of 2,000,000 stroops
    sqlx::query(
        "INSERT INTO withdrawals (merchant_id, amount_stroops, asset, status, bank_code, account_number, created_at, updated_at)
         VALUES ($1::uuid, 2_000_000, 'cNGN', 'completed', '058', '0123456789', now(), now())",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    // Attempting 3,000,000 (total 5,000,000 == 5,000,000 limit) -> allowed
    let (status, json) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 3_000_000,
            "asset": "cNGN",
            "bank_code": "058",
            "account_number": "0123456789"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "expected withdrawal success: {json}");

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 7_000_000);
}

#[tokio::test]
async fn withdrawal_daily_limit_ignores_past_days_and_failed_withdrawals() {
    let Some(mut state) = state().await else {
        return;
    };
    state.daily_withdrawal_limit_stroops = Some(3_000_000);
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "daily_limit_past_and_failed").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 10_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let past_time = chrono::Utc::now() - chrono::Duration::days(2);

    // Completed withdrawal 2 days ago of 3,000,000 stroops
    sqlx::query(
        "INSERT INTO withdrawals (merchant_id, amount_stroops, asset, status, bank_code, account_number, created_at, updated_at)
         VALUES ($1::uuid, 3_000_000, 'cNGN', 'completed', '058', '0123456789', $2, $2)",
    )
    .bind(&merchant_id)
    .bind(past_time)
    .execute(&state.db)
    .await
    .unwrap();

    // Failed withdrawal today of 3,000,000 stroops
    sqlx::query(
        "INSERT INTO withdrawals (merchant_id, amount_stroops, asset, status, failure_reason, bank_code, account_number, created_at, updated_at)
         VALUES ($1::uuid, 3_000_000, 'cNGN', 'failed', 'declined', '058', '0123456789', now(), now())",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    // Attempting 2,000,000 today (today's completed sum is 0 <= 3,000,000 limit) -> allowed
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
    assert_eq!(status, StatusCode::OK, "expected withdrawal success: {json}");

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 8_000_000);
}
