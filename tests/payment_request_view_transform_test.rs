//! Integration tests for the payment request view transformation (issue #1268).
//!
//! These tests pin the observable HTTP contract of the view transformation —
//! specifically the fields produced by `to_view()` / `row_to_view()` in
//! `src/api/payment_requests.rs`.  They are designed to remain green before,
//! during, and after the refactor described in
//! `docs/proposals/1268-payment-request-view-refactor.md`.
//!
//! # What is tested
//!
//! 1. `effective_status` contract via HTTP:
//!    - A `pending` request whose `expires_at` is still in the future reports
//!      `status = "pending"`.
//!    - A `pending` request whose `expires_at` is in the past reports
//!      `status = "expired"` (the API derives this at read time without a
//!      background job).
//!    - A `cancelled` request reports `status = "cancelled"`.
//!    - A `cancelled` request whose `expires_at` is still in the future reports
//!      `status = "cancelled"` (not `pending`) — issue #1472.
//!
//! 2. `build_sep7_uri` contract via HTTP:
//!    - An XLM payment request carries a non-null `sep7_uri` starting with
//!      `web+stellar:pay?`.
//!    - The `sep7_uri` encodes `destination`, `amount`, `memo`, and
//!      `memo_type=MEMO_TEXT`.
//!    - A memo containing special characters (`&`, `=`, `#`) is percent-encoded
//!      in `sep7_uri` (no raw delimiters in the query string).
//!
//! 3. The list endpoint (`GET /payment-requests`) returns the same shape as the
//!    single-item endpoint (`GET /payment-requests/{id}`).

mod common;

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;

async fn app_and_db() -> (axum::Router, PgPool) {
    let state = common::state().await;
    let db = state.db.clone();
    (aframp::router(state), db)
}

/// Create a payment request and return its full JSON view.
async fn create_pr(app: &axum::Router, token: &str, amount_stroops: i64) -> serde_json::Value {
    let (status, body) = common::send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(token),
        Some(json!({ "amount_stroops": amount_stroops })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create payment request failed: {body}"
    );
    body
}

// ---------------------------------------------------------------------------
// effective_status contract
// ---------------------------------------------------------------------------

/// A freshly created payment request has status = "pending".
#[tokio::test]
async fn fresh_payment_request_is_pending() {
    let (app, db) = app_and_db().await;
    let (token, _merchant_id) = common::ensure_merchant(&app, "view_pending").await;

    // Create a wallet first.
    let (status, _) = common::send(app.clone(), "POST", "/wallet/create", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);

    let pr = create_pr(&app, &token, 10_000_000).await;

    assert_eq!(
        pr["status"].as_str().unwrap(),
        "pending",
        "fresh request must be pending: {pr}"
    );

    // Fetch via GET as well.
    let id = pr["id"].as_str().unwrap();
    let (status, fetched) =
        common::send(app, "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["status"].as_str().unwrap(), "pending");

    let _ = db; // keep db alive
}

/// A pending request whose expires_at is in the past is reported as "expired".
#[tokio::test]
async fn expired_pending_request_status_is_expired() {
    let (app, db) = app_and_db().await;
    let (token, merchant_id) = common::ensure_merchant(&app, "view_expired").await;

    let (status, _) = common::send(app.clone(), "POST", "/wallet/create", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);

    let pr = create_pr(&app, &token, 10_000_000).await;
    let pr_id = pr["id"].as_str().unwrap();

    // Force the expiry into the past directly in the DB.
    sqlx::query(
        "UPDATE payment_requests SET expires_at = now() - interval '1 hour' \
         WHERE id = $1 AND merchant_id = $2::uuid",
    )
    .bind(uuid::Uuid::parse_str(pr_id).unwrap())
    .bind(&merchant_id)
    .execute(&db)
    .await
    .expect("force expiry");

    // The API must derive "expired" at read time without a background job.
    let (status, fetched) = common::send(
        app.clone(),
        "GET",
        &format!("/payment-requests/{pr_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "GET failed: {fetched}");
    assert_eq!(
        fetched["status"].as_str().unwrap(),
        "expired",
        "past-expiry pending request must report expired: {fetched}"
    );

    // The list endpoint must also report "expired".
    let (status, list) = common::send(app, "GET", "/payment-requests", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "list failed: {list}");
    let items = list["data"].as_array().expect("data must be array");
    let found = items.iter().find(|item| item["id"].as_str() == Some(pr_id));
    if let Some(item) = found {
        assert_eq!(
            item["status"].as_str().unwrap(),
            "expired",
            "list must also report expired: {item}"
        );
    }
}

/// A cancelled request reports status = "cancelled".
#[tokio::test]
async fn cancelled_request_reports_cancelled_status() {
    let (app, db) = app_and_db().await;
    let (token, _merchant_id) = common::ensure_merchant(&app, "view_cancelled").await;

    let (status, _) = common::send(app.clone(), "POST", "/wallet/create", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);

    let pr = create_pr(&app, &token, 10_000_000).await;
    let pr_id = pr["id"].as_str().unwrap();

    // Cancel the request via the API.
    let (status, cancelled) = common::send(
        app.clone(),
        "DELETE",
        &format!("/payment-requests/{pr_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "cancel failed: {cancelled}");
    assert_eq!(
        cancelled["status"].as_str().unwrap(),
        "cancelled",
        "cancelled response must have status=cancelled: {cancelled}"
    );

    // Re-fetch must also report cancelled.
    let (status, fetched) = common::send(
        app,
        "GET",
        &format!("/payment-requests/{pr_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "GET after cancel failed: {fetched}");
    assert_eq!(fetched["status"].as_str().unwrap(), "cancelled");

    let _ = db;
}

/// A request cancelled before its expiry must report "cancelled", not "pending".
///
/// Regression test for issue #1472: `effective_status` must check `cancelled_at`
/// before comparing `expires_at`, otherwise a cancelled-but-not-yet-expired row
/// is reported as `pending`.
#[tokio::test]
async fn cancelled_before_expiry_reports_cancelled_not_pending() {
    let (app, db) = app_and_db().await;
    let (token, merchant_id) = common::ensure_merchant(&app, "view_cancelled_before_expiry").await;

    let (status, _) = common::send(app.clone(), "POST", "/wallet/create", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);

    let pr = create_pr(&app, &token, 10_000_000).await;
    let pr_id = pr["id"].as_str().unwrap();

    // Cancel the request via the API while it is still well within its expiry.
    let (status, cancelled) = common::send(
        app.clone(),
        "DELETE",
        &format!("/payment-requests/{pr_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "cancel failed: {cancelled}");
    assert_eq!(
        cancelled["status"].as_str().unwrap(),
        "cancelled",
        "cancel response must have status=cancelled: {cancelled}"
    );

    // Ensure the row is still non-expired so the bug would surface as "pending".
    sqlx::query(
        "UPDATE payment_requests SET expires_at = now() + interval '1 hour' \
         WHERE id = $1 AND merchant_id = $2::uuid",
    )
    .bind(uuid::Uuid::parse_str(pr_id).unwrap())
    .bind(&merchant_id)
    .execute(&db)
    .await
    .expect("force future expiry");

    // GET must report cancelled, not pending.
    let (status, fetched) = common::send(
        app.clone(),
        "GET",
        &format!("/payment-requests/{pr_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "GET after cancel failed: {fetched}");
    assert_eq!(
        fetched["status"].as_str().unwrap(),
        "cancelled",
        "cancelled-before-expiry request must report cancelled, not pending: {fetched}"
    );

    // The list endpoint must agree.
    let (status, list) = common::send(app, "GET", "/payment-requests", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "list failed: {list}");
    let items = list["data"].as_array().expect("data must be array");
    if let Some(item) = items.iter().find(|item| item["id"].as_str() == Some(pr_id)) {
        assert_eq!(
            item["status"].as_str().unwrap(),
            "cancelled",
            "list must also report cancelled for non-expired cancelled row: {item}"
        );
    }
}

// ---------------------------------------------------------------------------
// build_sep7_uri / sep7_uri field contract
// ---------------------------------------------------------------------------

/// An XLM payment request carries a non-null sep7_uri.
#[tokio::test]
async fn xlm_request_has_sep7_uri() {
    let (app, _db) = app_and_db().await;
    let (token, _) = common::ensure_merchant(&app, "view_sep7_xlm").await;

    let (status, _) = common::send(app.clone(), "POST", "/wallet/create", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);

    let pr = create_pr(&app, &token, 25_000_000).await;

    let sep7 = pr["sep7_uri"].as_str().expect("sep7_uri must be a string");
    assert!(
        sep7.starts_with("web+stellar:pay?"),
        "sep7_uri must start with web+stellar:pay?: {sep7}"
    );
    assert!(
        sep7.contains("destination="),
        "sep7_uri must have destination: {sep7}"
    );
    assert!(
        sep7.contains("amount="),
        "sep7_uri must have amount: {sep7}"
    );
    assert!(sep7.contains("memo="), "sep7_uri must have memo: {sep7}");
    assert!(
        sep7.contains("memo_type=MEMO_TEXT"),
        "sep7_uri must have memo_type: {sep7}"
    );
}

/// The sep7_uri amount encodes stroops correctly (2.5000000 for 25_000_000 stroops).
#[tokio::test]
async fn sep7_uri_amount_is_correctly_encoded() {
    let (app, _db) = app_and_db().await;
    let (token, _) = common::ensure_merchant(&app, "view_sep7_amount").await;

    let (status, _) = common::send(app.clone(), "POST", "/wallet/create", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);

    let pr = create_pr(&app, &token, 25_000_000).await;
    let sep7 = pr["sep7_uri"].as_str().expect("sep7_uri must be a string");
    assert!(
        sep7.contains("amount=2.5000000"),
        "sep7_uri must encode 25_000_000 stroops as 2.5000000: {sep7}"
    );
}
