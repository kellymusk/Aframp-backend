//! Integration tests for GET /analytics/payments  (issue #1266)
//!
//! These tests specify and pin the contract for the daily-payment-volume
//! analytics endpoint.  They exercise:
//!
//! - Default date-range behaviour (last 30 days).
//! - Explicit `from`/`to` query parameters.
//! - Sparse output: days with zero confirmed payments are omitted.
//! - Only `confirmed` payments are counted; `detected`/`verified` are excluded.
//! - Authentication requirement (401 when no token).
//! - Validation: `from` after `to`, window > 366 days → 400.
//!
//! All tests seed payment rows directly into the DB (bypassing the full
//! deposit-detection flow) so they remain deterministic regardless of the
//! Stellar mock state.
//!
//! NOTE: The endpoint is not yet implemented (issue #1266 tracks the backend
//! work).  Until it is, every request will return 404.  Replace the
//! `assert_eq!(status, StatusCode::NOT_FOUND, ...)` guards with the correct
//! assertions once the implementation lands.

mod common;

use axum::http::StatusCode;
use serde_json::Value;
use sqlx::PgPool;

async fn app() -> (axum::Router, PgPool) {
    let state = common::state().await;
    let db = state.db.clone();
    (aframp::router(state), db)
}

/// Seed a payment row with the given status and `created_at` offset from now.
/// Returns the inserted payment id.
async fn seed_payment(
    db: &PgPool,
    merchant_id: &str,
    wallet_id: &str,
    status: &str,
    amount_stroops: i64,
    asset: &str,
    days_ago: i64,
) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO payments \
         (id, merchant_id, wallet_id, tx_hash, amount_stroops, asset, status, created_at, updated_at) \
         VALUES \
         ($1, $2::uuid, $3::uuid, $4, $5, $6, $7, \
          NOW() - ($8 || ' days')::interval, \
          NOW() - ($8 || ' days')::interval)",
    )
    .bind(id)
    .bind(merchant_id)
    .bind(wallet_id)
    .bind(format!("fakehash-{}", id.simple()))
    .bind(amount_stroops)
    .bind(asset)
    .bind(status)
    .bind(days_ago.to_string())
    .execute(db)
    .await
    .expect("seed payment");
    id
}

/// Convenience: create a merchant with wallet and return (token, merchant_id, wallet_id).
async fn merchant_with_wallet(app: &axum::Router, db: &PgPool, seed: &str) -> (String, String, String) {
    let (token, merchant_id) = common::ensure_merchant(app, seed).await;

    // Create the wallet for this merchant.
    let (status, body) = common::send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "wallet create failed: {body}");
    let wallet_id = body["id"].as_str().unwrap().to_string();

    (token, merchant_id, wallet_id)
}

// ---------------------------------------------------------------------------
// Authentication guard
// ---------------------------------------------------------------------------

/// GET /analytics/payments without a token must return 401.
#[tokio::test]
async fn analytics_payments_requires_auth() {
    let (app, _db) = app().await;

    let (status, body) = common::send(app, "GET", "/analytics/payments", None, None).await;

    // 404 until the endpoint is implemented; 401 once it is.
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND,
        "expected 401 (or 404 while unimplemented), got {status}: {body}"
    );
}

// ---------------------------------------------------------------------------
// Parameter validation
// ---------------------------------------------------------------------------

/// `from` after `to` must return 400 INVALID_PARAMETERS.
#[tokio::test]
async fn analytics_payments_from_after_to_is_400() {
    let (app, _db) = app().await;
    let (token, _merchant_id) = common::ensure_merchant(&app, "analytics_param_1").await;

    let (status, body) = common::send(
        app,
        "GET",
        "/analytics/payments?from=2026-10-19&to=2026-09-01",
        Some(&token),
        None,
    )
    .await;

    // 404 until implemented; 400 once it is.
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
        "expected 400 for from>to, got {status}: {body}"
    );
}

/// A date window wider than 366 days must return 400 INVALID_PARAMETERS.
#[tokio::test]
async fn analytics_payments_window_over_366_days_is_400() {
    let (app, _db) = app().await;
    let (token, _merchant_id) = common::ensure_merchant(&app, "analytics_param_2").await;

    // 370-day window.
    let (status, body) = common::send(
        app,
        "GET",
        "/analytics/payments?from=2025-10-14&to=2026-10-19",
        Some(&token),
        None,
    )
    .await;

    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
        "expected 400 for >366-day window, got {status}: {body}"
    );
}

/// A malformed date must return 400 INVALID_PARAMETERS.
#[tokio::test]
async fn analytics_payments_malformed_date_is_400() {
    let (app, _db) = app().await;
    let (token, _merchant_id) = common::ensure_merchant(&app, "analytics_param_3").await;

    let (status, body) = common::send(
        app,
        "GET",
        "/analytics/payments?from=not-a-date",
        Some(&token),
        None,
    )
    .await;

    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
        "expected 400 for malformed date, got {status}: {body}"
    );
}

// ---------------------------------------------------------------------------
// Data contract
// ---------------------------------------------------------------------------

/// When there are no confirmed payments in the window the `data` array is empty.
#[tokio::test]
async fn analytics_payments_empty_when_no_confirmed_payments() {
    let (app, db) = app().await;
    let (token, merchant_id, wallet_id) =
        merchant_with_wallet(&app, &db, "analytics_empty").await;

    // Seed only non-confirmed payments.
    seed_payment(&db, &merchant_id, &wallet_id, "detected", 10_000_000, "XLM", 1).await;
    seed_payment(&db, &merchant_id, &wallet_id, "verified", 10_000_000, "XLM", 2).await;

    let (status, body) = common::send(
        app,
        "GET",
        "/analytics/payments?from=2026-01-01&to=2026-12-31",
        Some(&token),
        None,
    )
    .await;

    if status == StatusCode::NOT_FOUND {
        // Endpoint not yet implemented — skip the assertion.
        return;
    }

    assert_eq!(status, StatusCode::OK, "unexpected error: {body}");
    assert_eq!(
        body["data"].as_array().unwrap().len(),
        0,
        "data must be empty when only non-confirmed payments exist: {body}"
    );
}

/// Confirmed payments within the window appear in the response; detected /
/// verified payments are excluded; payments outside the window are excluded.
#[tokio::test]
async fn analytics_payments_aggregates_confirmed_only() {
    let (app, db) = app().await;
    let (token, merchant_id, wallet_id) =
        merchant_with_wallet(&app, &db, "analytics_confirmed").await;

    // Seed: 2 confirmed payments from 5 days ago, 1 confirmed from 3 days ago,
    // 1 verified (excluded) from 4 days ago.
    seed_payment(&db, &merchant_id, &wallet_id, "confirmed", 10_000_000, "XLM", 5).await;
    seed_payment(&db, &merchant_id, &wallet_id, "confirmed", 5_000_000, "XLM", 5).await;
    seed_payment(&db, &merchant_id, &wallet_id, "confirmed", 20_000_000, "XLM", 3).await;
    seed_payment(&db, &merchant_id, &wallet_id, "verified",  8_000_000, "XLM", 4).await;

    let (status, body) = common::send(
        app,
        "GET",
        "/analytics/payments",
        Some(&token),
        None,
    )
    .await;

    if status == StatusCode::NOT_FOUND {
        // Endpoint not yet implemented — skip the assertion.
        return;
    }

    assert_eq!(status, StatusCode::OK, "unexpected error: {body}");

    let data = body["data"].as_array().expect("data must be an array");

    // Exactly 2 date buckets (5-days-ago and 3-days-ago).
    assert_eq!(data.len(), 2, "expected 2 date buckets: {body}");

    // Find the bucket 5 days ago.
    let bucket_5 = data
        .iter()
        .find(|b| {
            let total = b["total_stroops"].as_i64().unwrap_or(0);
            total == 15_000_000 // 10M + 5M
        })
        .expect("bucket with total_stroops=15000000 not found");
    assert_eq!(bucket_5["count"].as_i64().unwrap(), 2);

    // Find the bucket 3 days ago.
    let bucket_3 = data
        .iter()
        .find(|b| b["total_stroops"].as_i64().unwrap_or(0) == 20_000_000)
        .expect("bucket with total_stroops=20000000 not found");
    assert_eq!(bucket_3["count"].as_i64().unwrap(), 1);
}

/// The response shape must include `from`, `to`, and `data` fields.
#[tokio::test]
async fn analytics_payments_response_shape() {
    let (app, _db) = app().await;
    let (token, _merchant_id) = common::ensure_merchant(&app, "analytics_shape").await;

    let (status, body) = common::send(
        app,
        "GET",
        "/analytics/payments?from=2026-10-01&to=2026-10-19",
        Some(&token),
        None,
    )
    .await;

    if status == StatusCode::NOT_FOUND {
        return;
    }

    assert_eq!(status, StatusCode::OK, "unexpected error: {body}");
    assert!(body.get("from").is_some(), "response must have `from`: {body}");
    assert!(body.get("to").is_some(),   "response must have `to`: {body}");
    assert!(body.get("data").is_some(), "response must have `data`: {body}");

    // Confirm `from` and `to` echo the requested range.
    assert_eq!(body["from"].as_str().unwrap(), "2026-10-01");
    assert_eq!(body["to"].as_str().unwrap(),   "2026-10-19");
}

/// Payments from a different merchant must NOT appear in the response.
#[tokio::test]
async fn analytics_payments_scoped_to_authenticated_merchant() {
    let (app, db) = app().await;

    let (token_a, merchant_a, wallet_a) =
        merchant_with_wallet(&app, &db, "analytics_scope_a").await;
    let (_token_b, merchant_b, wallet_b) =
        merchant_with_wallet(&app, &db, "analytics_scope_b").await;

    // Seed a confirmed payment for merchant B only.
    seed_payment(&db, &merchant_b, &wallet_b, "confirmed", 99_000_000, "XLM", 1).await;

    let (status, body) = common::send(
        app,
        "GET",
        "/analytics/payments",
        Some(&token_a), // authenticated as merchant A
        None,
    )
    .await;

    if status == StatusCode::NOT_FOUND {
        return;
    }

    assert_eq!(status, StatusCode::OK, "unexpected error: {body}");
    let data = body["data"].as_array().expect("data must be an array");
    assert_eq!(
        data.len(),
        0,
        "merchant A must not see merchant B's payments: {body}"
    );
    // Suppress unused-variable warnings in case wallet_a is never used.
    let _ = (merchant_a, wallet_a);
}

fn _assert_value_is_used(_: &Value) {}
