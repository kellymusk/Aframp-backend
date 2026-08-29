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

    async fn verify_payout(&self, _reference: &str) -> Result<PayoutVerification, String> {
        Err("simulated provider failure".into())
    }
}

struct MockVerificationProvider {
    verification: PayoutVerification,
}

#[async_trait]
impl PaymentProvider for MockVerificationProvider {
    async fn create_payout(&self, req: &PayoutRequest) -> Result<PayoutResult, String> {
        Ok(PayoutResult {
            provider: "mock".into(),
            provider_reference: format!("mock_{}", req.reference),
            status: "pending".into(),
        })
    }

    async fn verify_payout(&self, _reference: &str) -> Result<PayoutVerification, String> {
        Ok(self.verification.clone())
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
async fn reconciliation_completed_marks_status_completed() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (_, merchant_id) = ensure_merchant(&app, "reconcile_ok").await;

    let withdrawal_id = uuid::Uuid::new_v4();
    let past = chrono::Utc::now() - chrono::Duration::minutes(15);

    sqlx::query(
        "INSERT INTO withdrawals (id, merchant_id, amount_stroops, asset, status, bank_code, account_number, created_at, updated_at)
         VALUES ($1, $2::uuid, 2_000_000, 'cNGN', 'pending', '058', '0123456789', $3, $3)",
    )
    .bind(withdrawal_id)
    .bind(&merchant_id)
    .bind(past)
    .execute(&state.db)
    .await
    .unwrap();

    let provider = MockVerificationProvider {
        verification: PayoutVerification::Completed {
            provider: "paystack".into(),
            provider_reference: "TRF_test_completed".into(),
        },
    };

    let report = aframp::services::withdrawals::reconcile_pending_withdrawals(
        &state.db,
        &provider,
    )
    .await
    .unwrap();

    assert!(report.completed >= 1);

    let row = sqlx::query_as::<_, aframp::models::Withdrawal>(
        "SELECT * FROM withdrawals WHERE id = $1",
    )
    .bind(withdrawal_id)
    .fetch_one(&state.db)
    .await
    .unwrap();

    assert_eq!(row.status, "completed");
    assert_eq!(row.provider.as_deref(), Some("paystack"));
    assert_eq!(row.provider_reference.as_deref(), Some("TRF_test_completed"));
}

#[tokio::test]
async fn reconciliation_failed_refunds_balance() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (_, merchant_id) = ensure_merchant(&app, "reconcile_fail").await;

    // Seed balance after debit: 3,000,000 (was 5,000,000 before a 2,000,000 withdrawal)
    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 3_000_000, 0)
         ON CONFLICT (merchant_id, asset) DO UPDATE SET available = 3_000_000, pending = 0",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let withdrawal_id = uuid::Uuid::new_v4();
    let past = chrono::Utc::now() - chrono::Duration::minutes(15);

    sqlx::query(
        "INSERT INTO withdrawals (id, merchant_id, amount_stroops, asset, status, bank_code, account_number, created_at, updated_at)
         VALUES ($1, $2::uuid, 2_000_000, 'cNGN', 'pending', '058', '0123456789', $3, $3)",
    )
    .bind(withdrawal_id)
    .bind(&merchant_id)
    .bind(past)
    .execute(&state.db)
    .await
    .unwrap();

    let provider = MockVerificationProvider {
        verification: PayoutVerification::Failed {
            provider: "paystack".into(),
            provider_reference: Some("TRF_test_failed".into()),
            reason: "Paystack transfer status: failed".into(),
        },
    };

    let report = aframp::services::withdrawals::reconcile_pending_withdrawals(
        &state.db,
        &provider,
    )
    .await
    .unwrap();

    assert!(report.failed_and_refunded >= 1);

    let row = sqlx::query_as::<_, aframp::models::Withdrawal>(
        "SELECT * FROM withdrawals WHERE id = $1",
    )
    .bind(withdrawal_id)
    .fetch_one(&state.db)
    .await
    .unwrap();

    assert_eq!(row.status, "failed");
    assert_eq!(row.failure_reason.as_deref(), Some("Paystack transfer status: failed"));

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 5_000_000, "balance must be refunded back to 5_000_000");
}

#[tokio::test]
async fn reconciliation_not_found_refunds_balance() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (_, merchant_id) = ensure_merchant(&app, "reconcile_notfound").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 3_000_000, 0)
         ON CONFLICT (merchant_id, asset) DO UPDATE SET available = 3_000_000, pending = 0",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let withdrawal_id = uuid::Uuid::new_v4();
    let past = chrono::Utc::now() - chrono::Duration::minutes(20);

    sqlx::query(
        "INSERT INTO withdrawals (id, merchant_id, amount_stroops, asset, status, bank_code, account_number, created_at, updated_at)
         VALUES ($1, $2::uuid, 2_000_000, 'cNGN', 'pending', '058', '0123456789', $3, $3)",
    )
    .bind(withdrawal_id)
    .bind(&merchant_id)
    .bind(past)
    .execute(&state.db)
    .await
    .unwrap();

    let provider = MockVerificationProvider {
        verification: PayoutVerification::NotFound,
    };

    let report = aframp::services::withdrawals::reconcile_pending_withdrawals(
        &state.db,
        &provider,
    )
    .await
    .unwrap();

    assert!(report.failed_and_refunded >= 1);

    let row = sqlx::query_as::<_, aframp::models::Withdrawal>(
        "SELECT * FROM withdrawals WHERE id = $1",
    )
    .bind(withdrawal_id)
    .fetch_one(&state.db)
    .await
    .unwrap();

    assert_eq!(row.status, "failed");
    assert!(row.failure_reason.unwrap().contains("not found"));

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 5_000_000, "balance must be refunded");
}

#[tokio::test]
async fn reconciliation_skips_recent_pending_withdrawals() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (_, merchant_id) = ensure_merchant(&app, "reconcile_recent").await;

    let withdrawal_id = uuid::Uuid::new_v4();
    // Only 2 minutes old (< 10 minutes)
    let recent = chrono::Utc::now() - chrono::Duration::minutes(2);

    sqlx::query(
        "INSERT INTO withdrawals (id, merchant_id, amount_stroops, asset, status, bank_code, account_number, created_at, updated_at)
         VALUES ($1, $2::uuid, 2_000_000, 'cNGN', 'pending', '058', '0123456789', $3, $3)",
    )
    .bind(withdrawal_id)
    .bind(&merchant_id)
    .bind(recent)
    .execute(&state.db)
    .await
    .unwrap();

    let provider = MockVerificationProvider {
        verification: PayoutVerification::Completed {
            provider: "paystack".into(),
            provider_reference: "TRF_should_not_run".into(),
        },
    };

    let report = aframp::services::withdrawals::reconcile_pending_withdrawals(
        &state.db,
        &provider,
    )
    .await
    .unwrap();

    // Check that recent withdrawal was not reconciled
    let row = sqlx::query_as::<_, aframp::models::Withdrawal>(
        "SELECT * FROM withdrawals WHERE id = $1",
    )
    .bind(withdrawal_id)
    .fetch_one(&state.db)
    .await
    .unwrap();

    assert_eq!(row.status, "pending", "recent pending withdrawal must stay pending");
    assert_eq!(row.provider_reference, None);
}
