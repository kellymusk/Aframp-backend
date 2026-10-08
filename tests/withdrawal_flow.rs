mod common;

use std::sync::Arc;

use aframp::payments::mock::MockProvider;
use aframp::payments::{PayoutVerification, PaymentProvider, PayoutRequest, PayoutResult};
use async_trait::async_trait;
use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, state};

/// Reconciliation scans every pending withdrawal in the database, so tests
/// that run it must not overlap or they reconcile each other's rows.
static RECONCILE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct FailingProvider;

#[async_trait]
impl PaymentProvider for FailingProvider {
    async fn resolve_account(&self, _bank_code: &str, _account_number: &str) -> Result<String, String> {
        Ok("Test Account Holder".into())
    }

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
    async fn resolve_account(&self, _bank_code: &str, _account_number: &str) -> Result<String, String> {
        Ok("Test Account Holder".into())
    }

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

/// Succeeds at the provider but removes the pending row first, so recording
/// the payout can never succeed — a stand-in for a persistent DB failure.
struct RowVanishingProvider(sqlx::PgPool);

#[async_trait]
impl PaymentProvider for RowVanishingProvider {
    async fn resolve_account(&self, _bank_code: &str, _account_number: &str) -> Result<String, String> {
        Ok("Test Account Holder".into())
    }

    async fn create_payout(&self, req: &PayoutRequest) -> Result<PayoutResult, String> {
        sqlx::query("DELETE FROM withdrawals WHERE id = $1::uuid")
            .bind(&req.reference)
            .execute(&self.0)
            .await
            .map_err(|e| e.to_string())?;
        Ok(PayoutResult {
            provider: "paystack".into(),
            provider_reference: "TRF_vanished".into(),
            status: "processing".into(),
        })
    }

    async fn verify_payout(&self, _reference: &str) -> Result<aframp::payments::PayoutVerification, String> {
        Ok(aframp::payments::PayoutVerification::NotFound)
    }
}

/// Simulates the Paystack error returned when the destination bank code
/// isn't a recognized institution code.
struct InvalidBankCodeProvider;

#[async_trait]
impl PaymentProvider for InvalidBankCodeProvider {
    async fn resolve_account(&self, _bank_code: &str, _account_number: &str) -> Result<String, String> {
        Ok("Test Account Holder".into())
    }

    async fn create_payout(&self, _req: &PayoutRequest) -> Result<PayoutResult, String> {
        Err("Invalid bank code".into())
    }

    async fn verify_payout(&self, _reference: &str) -> Result<aframp::payments::PayoutVerification, String> {
        Ok(aframp::payments::PayoutVerification::NotFound)
    }
}

/// Simulates the Paystack error returned when the account number doesn't
/// resolve for the given bank.
struct InvalidAccountNumberProvider;

#[async_trait]
impl PaymentProvider for InvalidAccountNumberProvider {
    async fn resolve_account(&self, _bank_code: &str, _account_number: &str) -> Result<String, String> {
        Ok("Test Account Holder".into())
    }

    async fn create_payout(&self, _req: &PayoutRequest) -> Result<PayoutResult, String> {
        Err("Could not resolve account number".into())
    }

    async fn verify_payout(&self, _reference: &str) -> Result<aframp::payments::PayoutVerification, String> {
        Ok(aframp::payments::PayoutVerification::NotFound)
    }
}

/// Simulates the request to Paystack timing out before a response is
/// received.
struct TimeoutProvider;

#[async_trait]
impl PaymentProvider for TimeoutProvider {
    async fn resolve_account(&self, _bank_code: &str, _account_number: &str) -> Result<String, String> {
        Ok("Test Account Holder".into())
    }

    async fn create_payout(&self, _req: &PayoutRequest) -> Result<PayoutResult, String> {
        Err("request to Paystack timed out".into())
    }

    async fn verify_payout(&self, _reference: &str) -> Result<aframp::payments::PayoutVerification, String> {
        Ok(aframp::payments::PayoutVerification::NotFound)
    }
}

#[tokio::test]
async fn withdrawal_insufficient_balance_rejected() {
    let state = state().await;
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
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "expected rejection: {json}"
    );
    assert_eq!(json["error"], "insufficient available balance");
}

#[tokio::test]
async fn withdrawal_validates_bank_details() {
    let state = state().await;
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
    let state = state().await;
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
    let withdrawals = json["data"].as_array().unwrap();
    assert_eq!(withdrawals.len(), 1);
    assert_eq!(withdrawals[0]["account_number"], "****6789");
}

#[tokio::test]
async fn withdrawal_full_balance_then_insufficient() {
    let state = state().await;
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
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "second withdrawal should fail"
    );
}

#[tokio::test]
async fn withdrawal_unsupported_asset_rejected() {
    let state = state().await;
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
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "expected rejection: {json}"
    );
    assert_eq!(
        json["error"],
        "withdrawals are only supported for the cNGN asset"
    );
}

#[tokio::test]
async fn withdrawal_rejects_sub_kobo_precision() {
    let state = state().await;
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
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "expected rejection: {json}"
    );
    assert_eq!(
        json["error"],
        "amount_stroops must be a whole number of kobo"
    );
}

#[tokio::test]
async fn withdrawal_payout_failure_refunds_balance_and_records_reason() {
    let mut state = state().await;
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
    assert_eq!(
        status,
        StatusCode::BAD_GATEWAY,
        "expected payout failure: {json}"
    );
    assert_eq!(json["error"], "simulated provider failure");
    assert_eq!(
        json["code"], "PAYOUT_FAILED",
        "502 shape must expose stable PAYOUT_FAILED code: {json}"
    );
    assert!(
        json.get("token").is_none(),
        "error responses must not leak session material: {json}"
    );

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(
        balance, 5_000_000,
        "balance should be refunded after a failed payout"
    );

    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let withdrawals = json["data"].as_array().unwrap();
    assert_eq!(
        withdrawals.len(),
        1,
        "the failed attempt should still leave an audit-trail row"
    );
    assert_eq!(withdrawals[0]["status"], "failed");
    assert_eq!(
        withdrawals[0]["failure_reason"],
        "simulated provider failure"
    );
}

/// #1129 — when MockProvider (or any provider) errors, the HTTP body is the
/// documented `{ error, code: PAYOUT_FAILED }` shape at 502. Clients must not
/// retry: the withdrawal row is already persisted as `failed`.
#[tokio::test]
async fn withdrawal_paystack_failure_returns_documented_502_payout_failed_shape() {
    let mut state = state().await;
    state.payment_provider = Arc::new(FailingProvider);
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "payout_502_shape").await;

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

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{json}");
    assert_eq!(json["code"], "PAYOUT_FAILED");
    assert!(json["error"].as_str().unwrap().contains("simulated provider failure"));
    assert_eq!(json.as_object().unwrap().len(), 2, "error body is only {{error, code}}: {json}");

    // Prove the row exists so frontends know a retry would duplicate work.
    let failed_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM withdrawals
          WHERE merchant_id = $1::uuid AND status = 'failed'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(failed_count, 1);
}

#[tokio::test]
async fn withdrawal_insufficient_balance_never_calls_provider() {
    let mut state = state().await;
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
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "expected rejection: {json}"
    );
    assert_eq!(json["error"], "insufficient available balance");
}

#[tokio::test]
async fn withdrawal_invalid_bank_code_refunds_balance_and_records_reason() {
    let mut state = state().await;
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
    assert_eq!(
        status,
        StatusCode::BAD_GATEWAY,
        "expected payout failure: {json}"
    );
    assert_eq!(json["error"], "Invalid bank code");

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(
        balance, 5_000_000,
        "balance should be refunded after an invalid bank code"
    );

    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let withdrawals = json["data"].as_array().unwrap();
    assert_eq!(withdrawals[0]["status"], "failed");
    assert_eq!(withdrawals[0]["failure_reason"], "Invalid bank code");
}

#[tokio::test]
async fn withdrawal_invalid_account_number_refunds_balance_and_records_reason() {
    let mut state = state().await;
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
    assert_eq!(
        status,
        StatusCode::BAD_GATEWAY,
        "expected payout failure: {json}"
    );
    assert_eq!(json["error"], "Could not resolve account number");

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(
        balance, 5_000_000,
        "balance should be refunded after an invalid account number"
    );

    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let withdrawals = json["data"].as_array().unwrap();
    assert_eq!(withdrawals[0]["status"], "failed");
    assert_eq!(
        withdrawals[0]["failure_reason"],
        "Could not resolve account number"
    );
}

#[tokio::test]
async fn withdrawal_paystack_timeout_refunds_balance_and_records_reason() {
    let mut state = state().await;
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
    assert_eq!(
        status,
        StatusCode::BAD_GATEWAY,
        "expected payout failure: {json}"
    );
    assert_eq!(json["error"], "request to Paystack timed out");

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(
        balance, 5_000_000,
        "balance should be refunded after a provider timeout"
    );

    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let withdrawals = json["data"].as_array().unwrap();
    assert_eq!(withdrawals[0]["status"], "failed");
    assert_eq!(
        withdrawals[0]["failure_reason"],
        "request to Paystack timed out"
    );
}

#[tokio::test]
async fn withdraw_amount_stroops_rejects_float_and_string() {
    let mut state = state().await;
    state.payment_provider = Arc::new(MockProvider);
    let app = aframp::router(state);
    let (token, _) = ensure_merchant(&app, "wd_amount_types").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 1.5,
            "bank_code": "058",
            "account_number": "0123456789",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "float should be 400: {json}");
    assert_eq!(json["code"], "INVALID_PARAMETERS");
    assert_eq!(json["field"], "amount_stroops");

    let (status, json) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": "1000000",
            "bank_code": "058",
            "account_number": "0123456789",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "string should be 400: {json}");
    assert_eq!(json["code"], "INVALID_PARAMETERS");
    assert_eq!(json["field"], "amount_stroops");
}

#[tokio::test]
async fn withdrawal_payout_record_failure_is_surfaced_not_refunded() {
    let mut state = state().await;
    state.payment_provider = Arc::new(RowVanishingProvider(state.db.clone()));
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "payout_record_fail").await;

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
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a payout that could not be recorded must surface as an error: {json}"
    );

    // The transfer went out, so the debit must stand — no refund.
    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 3_000_000);
}

#[tokio::test]
async fn reconciliation_completed_marks_status_completed() {
    let _reconcile = RECONCILE_LOCK.lock().await;
    let state = state().await;
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

    assert_eq!(row.status.as_str(), "completed");
    assert_eq!(row.provider.as_deref(), Some("paystack"));
    assert_eq!(row.provider_reference.as_deref(), Some("TRF_test_completed"));
}

#[tokio::test]
async fn reconciliation_failed_refunds_balance() {
    let _reconcile = RECONCILE_LOCK.lock().await;
    let state = state().await;
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

    assert_eq!(row.status.as_str(), "failed");
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
    let _reconcile = RECONCILE_LOCK.lock().await;
    let state = state().await;
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

    assert_eq!(row.status.as_str(), "failed");
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
    let _reconcile = RECONCILE_LOCK.lock().await;
    let state = state().await;
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

    assert_eq!(row.status.as_str(), "pending", "recent pending withdrawal must stay pending");
    assert_eq!(row.provider_reference, None);
}

#[tokio::test]
async fn withdrawal_daily_limit_exceeded_rejected() {
    let mut state = state().await;
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
    let mut state = state().await;
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
    let mut state = state().await;
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

/// Pending (not yet settled) withdrawals count toward the daily limit, so a
/// merchant can't stack in-flight payouts past it.
#[tokio::test]
async fn withdrawal_daily_limit_counts_pending_withdrawals() {
    let mut state = state().await;
    state.daily_withdrawal_limit_stroops = Some(3_000_000);
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "daily_limit_pending").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 10_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO withdrawals (merchant_id, amount_stroops, asset, status, bank_code, account_number)
         VALUES ($1::uuid, 2_000_000, 'cNGN', 'pending', '058', '0123456789')",
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
    assert_eq!(status, StatusCode::BAD_REQUEST, "pending withdrawal must count: {json}");
    assert_eq!(json["code"], "DAILY_LIMIT_EXCEEDED");
}

/// #1002 — fee preview: Paystack's flat ₦100 (10,000 kobo) fee and the net amount.
#[tokio::test]
async fn withdrawal_fee_endpoint_reports_fee_and_net() {
    let state = state().await;
    let app = aframp::router(state);
    let (status, json) = send(app.clone(), "GET", "/withdrawal-fee?amount_stroops=5000000000", None, None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["fee_stroops"], 1_000_000_000);
    assert_eq!(json["net_amount_stroops"], 4_000_000_000i64);

    let (status, json) = send(app, "GET", "/withdrawal-fee?amount_stroops=0", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
}

/// #1004 — GET /withdrawals/{id} is merchant-scoped: another merchant's id is a 404.
#[tokio::test]
async fn get_withdrawal_by_id_is_scoped_to_the_merchant() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "withdraw_get_by_id").await;
    let (other_token, _) = ensure_merchant(&app, "withdraw_get_by_id_other").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 5_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();
    let (status, created) = send(
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
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["id"].as_str().unwrap();

    let (status, fetched) = send(app.clone(), "GET", &format!("/withdrawals/{id}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{fetched}");
    assert_eq!(fetched["id"], id);
    assert_eq!(fetched["account_number"], "****6789");

    let (status, json) = send(app.clone(), "GET", &format!("/withdrawals/{id}"), Some(&other_token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(json["code"], "WITHDRAWAL_NOT_FOUND");
}
