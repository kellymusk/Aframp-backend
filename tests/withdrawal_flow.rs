mod common;

use std::sync::Arc;

use aframp::payments::mock::MockProvider;
use aframp::payments::{PaymentProvider, PayoutRequest, PayoutResult};
use async_trait::async_trait;
use axum::http::StatusCode;
use serde_json::json;

use common::{assert_no_sensitive_fields_json, ensure_merchant, send, state};

/// Seeds a cNGN balance row directly into the DB for the given merchant.
async fn seed_balance(state: &aframp::AppState, merchant_id: &str, available: i64) {
    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', $2, 0)
         ON CONFLICT (merchant_id, asset) DO UPDATE SET available = $2, pending = 0",
    )
    .bind(merchant_id)
    .bind(available)
    .execute(&state.db)
    .await
    .unwrap();
}

/// Marks a merchant as KYC-verified directly in the DB so withdrawal tests
/// exercise the payout path rather than the KYC gate.
async fn verify_merchant(state: &aframp::AppState, merchant_id: &str) {
    sqlx::query("UPDATE merchants SET kyc_status = 'verified' WHERE id = $1::uuid")
        .bind(merchant_id)
        .execute(&state.db)
        .await
        .unwrap();
}

struct FailingProvider;

#[async_trait]
impl PaymentProvider for FailingProvider {
    async fn create_payout(&self, _req: &PayoutRequest) -> Result<PayoutResult, String> {
        Err("simulated provider failure".into())
    }
}

/// Succeeds at the provider but removes the pending row first, so recording
/// the payout can never succeed — a stand-in for a persistent DB failure.
struct RowVanishingProvider(sqlx::PgPool);

#[async_trait]
impl PaymentProvider for RowVanishingProvider {
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

/// Simulates Paystack returning an `otp` status, which is not one of the
/// internal `withdrawals.status` values. The service must map it to a valid
/// internal status (`processing`) rather than writing it verbatim.
struct OtpStatusProvider;

#[async_trait]
impl PaymentProvider for OtpStatusProvider {
    async fn create_payout(&self, _req: &PayoutRequest) -> Result<PayoutResult, String> {
        Ok(PayoutResult {
            provider_reference: "ps_otp_ref".into(),
            status: "otp".into(),
        })
    }
}

/// Simulates Paystack returning a status string the service has never seen
/// before. The service must fall back to a valid internal status
/// (`processing`) and log a warning instead of failing the DB write.
struct UnknownStatusProvider;

#[async_trait]
impl PaymentProvider for UnknownStatusProvider {
    async fn create_payout(&self, _req: &PayoutRequest) -> Result<PayoutResult, String> {
        Ok(PayoutResult {
            provider_reference: "ps_unknown_ref".into(),
            status: "some_future_status".into(),
        })
    }
}

#[tokio::test]
async fn withdrawal_happy_path_debits_balance_and_records_provider_reference() {
    let Some(mut state) = state().await else {
        return;
    };
    // MockProvider always succeeds, so this exercises the full happy path.
    state.payment_provider = Arc::new(MockProvider);
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "happy_path").await;

    seed_balance(&state, &merchant_id, 5_000_000).await;

    // The seeded balance must actually be visible before we withdraw.
    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 5_000_000, "seeded balance should be non-zero");

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

    // Balance should be debited by exactly the withdrawal amount.
    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 3_000_000, "available balance should be debited by the withdrawal amount");

    // The persisted row should carry the provider's status and reference.
    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let withdrawals = json.as_array().unwrap();
    assert_eq!(withdrawals.len(), 1);
    assert_eq!(withdrawals[0]["status"], "pending");
    assert!(
        withdrawals[0]["provider_reference"].as_str().is_some_and(|r| !r.is_empty()),
        "provider_reference should be populated: {}",
        withdrawals[0]
    );
}

#[tokio::test]
async fn withdrawal_insufficient_balance_rejected() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "insufficient").await;
    verify_merchant(&state, &merchant_id).await;

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
    let (token, merchant_id) = ensure_merchant(&app, "validation").await;
    verify_merchant(&state, &merchant_id).await;

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
    verify_merchant(&state, &merchant_id).await;

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
    assert_no_sensitive_fields_json(&json);
    assert_eq!(json.as_array().unwrap().len(), 1);
    let withdrawals = json["data"].as_array().unwrap();
    assert_eq!(withdrawals.len(), 1);
    assert_eq!(withdrawals[0]["account_number"], "****6789");
}

#[tokio::test]
async fn withdrawal_full_balance_then_insufficient() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "drain").await;
    verify_merchant(&state, &merchant_id).await;

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
    verify_merchant(&state, &merchant_id).await;

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
    verify_merchant(&state, &merchant_id).await;

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
    verify_merchant(&state, &merchant_id).await;

    seed_balance(&state, &merchant_id, 5_000_000).await;

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
    let Some(mut state) = state().await else {
        return;
    };
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
async fn withdrawal_paystack_otp_status_maps_to_processing() {
    let Some(mut state) = state().await else {
        return;
    };
    state.payment_provider = Arc::new(OtpStatusProvider);
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "paystack_otp").await;

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
    assert_eq!(status, StatusCode::OK, "withdraw failed: {json}");

    // The provider returned "otp", which is not a valid internal status.
    // The row must have been updated to a valid internal status instead of
    // tripping the CHECK constraint.
    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_no_sensitive_fields_json(&json);
    let withdrawals = json.as_array().unwrap();
    assert_eq!(withdrawals.len(), 1);
    assert_eq!(
        withdrawals[0]["status"], "processing",
        "Paystack 'otp' should map to internal 'processing'"
    );
}

#[tokio::test]
async fn withdrawal_paystack_unknown_status_falls_back_to_processing() {
    let Some(mut state) = state().await else {
        return;
    };
    state.payment_provider = Arc::new(UnknownStatusProvider);
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "paystack_unknown").await;

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
    assert_eq!(status, StatusCode::OK, "withdraw failed: {json}");

    // An unrecognized provider status must not cause a constraint violation;
    // the service should warn and default to "processing".
    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let withdrawals = json.as_array().unwrap();
    assert_eq!(withdrawals.len(), 1);
    assert_eq!(
        withdrawals[0]["status"], "processing",
        "unknown provider status should fall back to internal 'processing'"
    );
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
    let (token, merchant_id) = ensure_merchant(&app, "insufficient_mock").await;
    verify_merchant(&state, &merchant_id).await;

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
    verify_merchant(&state, &merchant_id).await;

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
    assert_no_sensitive_fields_json(&json);
    let withdrawals = json.as_array().unwrap();
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
    verify_merchant(&state, &merchant_id).await;

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
    assert_no_sensitive_fields_json(&json);
    let withdrawals = json.as_array().unwrap();
    let withdrawals = json["data"].as_array().unwrap();
    assert_eq!(withdrawals[0]["status"], "failed");
    assert_eq!(
        withdrawals[0]["failure_reason"],
        "Could not resolve account number"
    );
}

#[tokio::test]
async fn withdrawal_idempotency_key_returns_original_withdrawal() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "idempotent").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 5_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let body = json!({
        "amount_stroops": 2_000_000,
        "asset": "cNGN",
        "bank_code": "058",
        "account_number": "0123456789"
    });

    let (status, first) = send_with_headers(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(body.clone()),
        &[("Idempotency-Key", "retry-key-1")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "first withdraw failed: {first}");

    // A retry with the same key and identical body must return the original
    // withdrawal rather than debiting the balance a second time.
    let (status, second) = send_with_headers(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(body.clone()),
        &[("Idempotency-Key", "retry-key-1")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "retry should return the original response: {second}");
    assert_eq!(first["id"], second["id"], "retry must return the original withdrawal id");

    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 3_000_000, "balance should be debited exactly once");

    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json.as_array().unwrap().len(), 1, "retry must not create a second withdrawal");

    // A different idempotency key is a distinct request and creates a new
    // withdrawal.
    let (status, third) = send_with_headers(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(body),
        &[("Idempotency-Key", "retry-key-2")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "second distinct withdraw failed: {third}");
    assert_ne!(first["id"], third["id"], "a different key must create a new withdrawal");

    let (status, json) = send(app.clone(), "GET", "/withdrawals", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json.as_array().unwrap().len(), 2, "distinct keys create distinct withdrawals");
}

#[tokio::test]
async fn withdrawal_paystack_timeout_refunds_balance_and_records_reason() {
    let mut state = state().await;
    state.payment_provider = Arc::new(TimeoutProvider);
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "paystack_timeout").await;
    verify_merchant(&state, &merchant_id).await;

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
    assert_no_sensitive_fields_json(&json);
    let withdrawals = json.as_array().unwrap();
    let withdrawals = json["data"].as_array().unwrap();
    assert_eq!(withdrawals[0]["status"], "failed");
    assert_eq!(
        withdrawals[0]["failure_reason"],
        "request to Paystack timed out"
    );
}

#[tokio::test]
async fn withdraw_amount_stroops_rejects_float_and_string() {
    let Some(mut state) = state().await else {
        return;
    };
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
    let Some(mut state) = state().await else {
        return;
    };
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
async fn withdrawal_blocked_for_unverified_merchant() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "kyc_unverified").await;

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
    assert_eq!(status, StatusCode::FORBIDDEN, "expected KYC gate: {json}");
    assert_eq!(json["error"], "merchant KYC verification required before withdrawals");

    // The balance must be untouched since the gate fires before any debit.
    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(balance, 5_000_000, "balance should not be debited when KYC gate blocks");
}

#[tokio::test]
async fn kyc_submission_sets_status_pending() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "kyc_submit").await;

    let (status, json) = send(
        app.clone(),
        "PUT",
        "/merchants/kyc",
        Some(&token),
        Some(json!({
            "bvn": "12345678901",
            "cac_number": "RC1234567"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "kyc submission failed: {json}");
    assert_eq!(json["kyc_status"], "pending");

    let kyc_status = sqlx::query_scalar::<_, String>(
        "SELECT kyc_status FROM merchants WHERE id = $1::uuid",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(kyc_status, "pending");

    // Still gated while pending.
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
    assert_eq!(status, StatusCode::FORBIDDEN, "pending KYC should still block: {json}");
}

#[tokio::test]
async fn admin_kyc_approval_allows_withdrawal() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "kyc_approve").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 5_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    // Submit KYC first so the merchant is in the pending state.
    let (status, _) = send(
        app.clone(),
        "PUT",
        "/merchants/kyc",
        Some(&token),
        Some(json!({
            "bvn": "12345678901",
            "cac_number": "RC1234567"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Admin approves.
    let (status, json) = send(
        app.clone(),
        "PATCH",
        &format!("/admin/merchants/{merchant_id}/kyc"),
        None,
        Some(json!({ "kyc_status": "verified" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "admin approval failed: {json}");
    assert_eq!(json["kyc_status"], "verified");

    // Withdrawal now succeeds.
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
    assert_eq!(status, StatusCode::OK, "withdraw failed after approval: {json}");
    assert_eq!(json["status"], "pending");
}

#[tokio::test]
async fn admin_kyc_rejection_blocks_withdrawal() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "kyc_reject").await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', 5_000_000, 0)",
    )
    .bind(&merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let (status, _) = send(
        app.clone(),
        "PUT",
        "/merchants/kyc",
        Some(&token),
        Some(json!({
            "bvn": "12345678901",
            "cac_number": "RC1234567"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, json) = send(
        app.clone(),
        "PATCH",
        &format!("/admin/merchants/{merchant_id}/kyc"),
        None,
        Some(json!({ "kyc_status": "rejected" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "admin rejection failed: {json}");
    assert_eq!(json["kyc_status"], "rejected");

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
    assert_eq!(status, StatusCode::FORBIDDEN, "rejected KYC should block: {json}");
}

#[tokio::test]
async fn me_surfaces_kyc_status() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "kyc_me").await;

    let (status, json) = send(app.clone(), "GET", "/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["kyc_status"], "unverified");

    verify_merchant(&state, &merchant_id).await;

    let (status, json) = send(app.clone(), "GET", "/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["kyc_status"], "verified");
}
