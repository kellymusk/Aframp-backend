//! The compensating transaction in `services/withdrawals.rs`: when the payout
//! provider fails after the debit has already committed, the balance must be
//! refunded and the withdrawal marked `failed` — atomically.

mod common;

use std::sync::Arc;

use aframp::payments::{PaymentProvider, PayoutRequest, PayoutResult};
use async_trait::async_trait;
use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;

use common::{ensure_merchant, send, state};

const PROVIDER_ERROR: &str = "mock paystack: transfer rejected";
const STARTING_BALANCE: i64 = 5_000_000;
const WITHDRAW_AMOUNT: i64 = 2_000_000;

/// Stand-in for Paystack rejecting the transfer. The shipped `MockProvider`
/// always succeeds, so the failure path needs its own mock.
struct FailingMockProvider;

#[async_trait]
impl PaymentProvider for FailingMockProvider {
    async fn create_payout(&self, _req: &PayoutRequest) -> Result<PayoutResult, String> {
        Err(PROVIDER_ERROR.into())
    }
}

/// A router whose payment provider always fails, plus a merchant with a
/// funded cNGN balance. Returns (app, db, token, merchant_id).
async fn funded_merchant_with_failing_provider(seed: &str) -> (axum::Router, PgPool, String, String) {
    let mut state = state().await;
    state.payment_provider = Arc::new(FailingMockProvider);
    let db = state.db.clone();
    let app = aframp::router(state);
    let (token, merchant_id) = ensure_merchant(&app, seed).await;

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1::uuid, 'cNGN', $2, 0)",
    )
    .bind(&merchant_id)
    .bind(STARTING_BALANCE)
    .execute(&db)
    .await
    .unwrap();

    (app, db, token, merchant_id)
}

async fn withdraw(app: &axum::Router, token: &str) -> (StatusCode, serde_json::Value) {
    send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(token),
        Some(json!({
            "amount_stroops": WITHDRAW_AMOUNT,
            "asset": "cNGN",
            "bank_code": "058",
            "account_number": "0123456789"
        })),
    )
    .await
}

async fn available(db: &PgPool, merchant_id: &str) -> i64 {
    sqlx::query_scalar("SELECT available FROM balances WHERE merchant_id = $1::uuid AND asset = 'cNGN'")
        .bind(merchant_id)
        .fetch_one(db)
        .await
        .unwrap()
}

/// Every withdrawal row for the merchant as (status, failure_reason).
async fn withdrawal_rows(db: &PgPool, merchant_id: &str) -> Vec<(String, Option<String>)> {
    sqlx::query_as("SELECT status, failure_reason FROM withdrawals WHERE merchant_id = $1::uuid")
        .bind(merchant_id)
        .fetch_all(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn provider_error_restores_balance() {
    let (app, db, token, merchant_id) = funded_merchant_with_failing_provider("refund_balance").await;

    let (status, json) = withdraw(&app, &token).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "expected payout failure: {json}");
    assert_eq!(json["code"], "PAYOUT_FAILED");

    assert_eq!(
        available(&db, &merchant_id).await,
        STARTING_BALANCE,
        "the debited amount must be refunded in full"
    );
}

#[tokio::test]
async fn provider_error_marks_withdrawal_failed_with_reason() {
    let (app, db, token, merchant_id) = funded_merchant_with_failing_provider("refund_status").await;

    let (status, json) = withdraw(&app, &token).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "expected payout failure: {json}");

    let rows = withdrawal_rows(&db, &merchant_id).await;
    assert_eq!(rows.len(), 1, "the failed attempt must leave exactly one audit row");
    let (status, failure_reason) = &rows[0];
    assert_eq!(status, "failed");
    assert_eq!(failure_reason.as_deref(), Some(PROVIDER_ERROR));
}

#[tokio::test]
async fn refund_is_atomic_balance_and_status_commit_together() {
    let (app, db, token, merchant_id) = funded_merchant_with_failing_provider("refund_atomic").await;

    // Sabotage only the second half of the refund transaction: a trigger that
    // rejects marking *this merchant's* withdrawal as failed. The balance
    // restore runs first and succeeds; if the two writes weren't in one
    // transaction, the balance would come back while the row stayed pending.
    // Scoped to this merchant's id so parallel tests are unaffected.
    let suffix = merchant_id.replace('-', "");
    let function = format!("aframp_test_block_refund_{suffix}");
    let trigger = format!("aframp_test_block_refund_{suffix}");
    sqlx::query(&format!(
        "CREATE FUNCTION {function}() RETURNS trigger AS $$
         BEGIN
           IF NEW.merchant_id = '{merchant_id}'::uuid AND NEW.status = 'failed' THEN
             RAISE EXCEPTION 'simulated failure while marking withdrawal failed';
           END IF;
           RETURN NEW;
         END
         $$ LANGUAGE plpgsql"
    ))
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(&format!(
        "CREATE TRIGGER {trigger} BEFORE UPDATE ON withdrawals
         FOR EACH ROW EXECUTE FUNCTION {function}()"
    ))
    .execute(&db)
    .await
    .unwrap();

    let (status, json) = withdraw(&app, &token).await;
    let balance = available(&db, &merchant_id).await;
    let rows = withdrawal_rows(&db, &merchant_id).await;

    // Clean up before asserting so a failure doesn't leave the trigger behind.
    sqlx::query(&format!("DROP TRIGGER {trigger} ON withdrawals"))
        .execute(&db)
        .await
        .unwrap();
    sqlx::query(&format!("DROP FUNCTION {function}()"))
        .execute(&db)
        .await
        .unwrap();

    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a failed refund transaction should surface as an internal error: {json}"
    );
    assert_eq!(rows.len(), 1, "the committed debit's audit row must survive");
    assert_eq!(
        rows[0].0, "pending",
        "the status update was rejected, so the row must remain pending"
    );
    assert_eq!(rows[0].1, None, "no failure_reason may be recorded when the refund rolled back");
    assert_eq!(
        balance,
        STARTING_BALANCE - WITHDRAW_AMOUNT,
        "the balance restore must roll back together with the rejected status update"
    );
}
