//! #1126 — GET /transactions, /withdrawals, and /payment-requests limit
//! boundary matrix: clamp 0→1 and 201→200, honour 1/200, default 50, and
//! reject non-numeric `limit` without a 500.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use uuid::Uuid;

use common::{ensure_merchant, send, state};

/// List endpoints return either a bare array (legacy) or a cursor page
/// `{ "data": [...], "next_cursor": ... }`. Prefer `data` when present.
fn page_items(json: &Value) -> &[Value] {
    if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
        return data.as_slice();
    }
    json.as_array().map(|a| a.as_slice()).unwrap_or(&[])
}

fn assert_bad_limit(status: StatusCode, json: &Value) {
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "non-numeric limit must be 400 (not 500): status={status} body={json}"
    );
    // Documented API error format (API.md): { "error": "...", "code": "..." }.
    // When the body is JSON, require that contract so regressions can't sneak
    // a plain-text or 500-shaped failure back in.
    if json.is_object() {
        assert!(
            json.get("error").and_then(|e| e.as_str()).is_some(),
            "API error must include human `error` string: {json}"
        );
        let code = json.get("code").and_then(|c| c.as_str());
        assert_eq!(
            code,
            Some("INVALID_PARAMETERS"),
            "bad limit must use INVALID_PARAMETERS: {json}"
        );
    } else {
        // Axum's default Query rejection is plain text today. Still not a 500,
        // which is the regression this issue guards against. See
        // docs/proposals/1126-query-limit-error-shape.md for the follow-up
        // that maps the rejection onto the documented JSON error shape.
        assert!(
            json.is_null() || json.is_string(),
            "unexpected body for limit=abc: {json}"
        );
    }
}

async fn ensure_wallet(app: &axum::Router, token: &str) -> String {
    let (status, json) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "wallet create failed: {json}");
    json["address"].as_str().unwrap().to_string()
}

async fn seed_payments(db: &sqlx::PgPool, merchant_id: &str, address: &str, n: i64) {
    let wallet_id: Uuid = sqlx::query_scalar("SELECT id FROM wallets WHERE address = $1")
        .bind(address)
        .fetch_one(db)
        .await
        .expect("wallet row");
    let merchant: Uuid = merchant_id.parse().unwrap();
    for i in 0..n {
        sqlx::query(
            "INSERT INTO payments
               (merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status)
             VALUES ($1, $2, $3, $4, 1000, 'XLM', 'stellar', 'confirmed')",
        )
        .bind(merchant)
        .bind(wallet_id)
        .bind(address)
        .bind(format!("lim_pay_{merchant_id}_{i}"))
        .execute(db)
        .await
        .unwrap_or_else(|e| panic!("seed payment {i}: {e}"));
    }
}

async fn seed_withdrawals(db: &sqlx::PgPool, merchant_id: &str, n: i64) {
    let merchant: Uuid = merchant_id.parse().unwrap();
    for i in 0..n {
        sqlx::query(
            "INSERT INTO withdrawals
               (merchant_id, amount_stroops, asset, status, bank_code, account_number)
             VALUES ($1, 100000, 'cNGN', 'failed', '058', '0123456789')",
        )
        .bind(merchant)
        .execute(db)
        .await
        .unwrap_or_else(|e| panic!("seed withdrawal {i}: {e}"));
    }
}

async fn seed_payment_requests(db: &sqlx::PgPool, merchant_id: &str, address: &str, n: i64) {
    let wallet_id: Uuid = sqlx::query_scalar("SELECT id FROM wallets WHERE address = $1")
        .bind(address)
        .fetch_one(db)
        .await
        .expect("wallet row");
    let merchant: Uuid = merchant_id.parse().unwrap();
    for i in 0..n {
        // memo is globally UNIQUE — include merchant id + index.
        let memo = format!("{}{i:04}", &merchant_id[..8]);
        sqlx::query(
            "INSERT INTO payment_requests
               (merchant_id, wallet_id, amount_stroops, asset, memo, status, expires_at)
             VALUES ($1, $2, 1000000, 'XLM', $3, 'pending', now() + interval '1 hour')",
        )
        .bind(merchant)
        .bind(wallet_id)
        .bind(&memo)
        .execute(db)
        .await
        .unwrap_or_else(|e| panic!("seed payment request {i}: {e}"));
    }
}

/// Returns the cursor / has_more marker from a page response, if any.
fn page_has_more(json: &Value) -> bool {
    if let Some(has_more) = json.get("has_more").and_then(|v| v.as_bool()) {
        return has_more;
    }
    if let Some(next) = json.get("next_cursor") {
        return !next.is_null();
    }
    false
}

async fn assert_limit_matrix(app: axum::Router, token: &str, path: &str) {
    // Missing limit → default 50
    let (status, json) = send(app.clone(), "GET", path, Some(token), None).await;
    assert_eq!(status, StatusCode::OK, "default limit on {path}: {json}");
    assert_eq!(
        page_items(&json).len(),
        50,
        "{path}: missing ?limit must default to 50"
    );

    // limit=0 → clamped to 1
    let (status, json) = send(
        app.clone(),
        "GET",
        &format!("{path}?limit=0"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{path}?limit=0: {json}");
    assert_eq!(
        page_items(&json).len(),
        1,
        "{path}: limit=0 must clamp to 1, not return 0 rows or error"
    );

    // limit=1
    let (status, json) = send(
        app.clone(),
        "GET",
        &format!("{path}?limit=1"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{path}?limit=1: {json}");
    assert_eq!(page_items(&json).len(), 1);

    // limit=200
    let (status, json) = send(
        app.clone(),
        "GET",
        &format!("{path}?limit=200"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{path}?limit=200: {json}");
    assert_eq!(page_items(&json).len(), 200);

    // limit=201 → clamped to 200
    let (status, json) = send(
        app.clone(),
        "GET",
        &format!("{path}?limit=201"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{path}?limit=201: {json}");
    assert_eq!(
        page_items(&json).len(),
        200,
        "{path}: limit=201 must clamp to 200"
    );

    // limit=abc → 400, never 500
    let (status, json) = send(
        app.clone(),
        "GET",
        &format!("{path}?limit=abc"),
        Some(token),
        None,
    )
    .await;
    assert_bad_limit(status, &json);
}

#[tokio::test]
async fn transactions_limit_boundaries() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "lim_tx").await;
    let address = ensure_wallet(&app, &token).await;
    seed_payments(&state.db, &merchant_id, &address, 210).await;
    assert_limit_matrix(app, &token, "/transactions").await;
}

#[tokio::test]
async fn withdrawals_limit_boundaries() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "lim_wd").await;
    let _address = ensure_wallet(&app, &token).await;
    seed_withdrawals(&state.db, &merchant_id, 210).await;
    assert_limit_matrix(app, &token, "/withdrawals").await;
}

#[tokio::test]
async fn payment_requests_limit_boundaries() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "lim_pr").await;
    let address = ensure_wallet(&app, &token).await;
    seed_payment_requests(&state.db, &merchant_id, &address, 210).await;
    assert_limit_matrix(app, &token, "/payment-requests").await;
}

/// #1496 — a merchant asking for `limit=500` must be clamped to the merchant
/// maximum (200), never the admin maximum (500).
#[tokio::test]
async fn merchant_limit_is_clamped_to_200() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "lim_merchant_cap").await;
    let address = ensure_wallet(&app, &token).await;
    seed_payments(&state.db, &merchant_id, &address, 510).await;

    let (status, json) = send(
        app.clone(),
        "GET",
        "/transactions?limit=500",
/// #1504 — documents the *current* behaviour: an out-of-range `limit`
/// (e.g. `limit=99999`) is silently clamped to 200 and the request succeeds
/// with no indication the value was ignored. See
/// docs/proposals/1126-query-limit-error-shape.md for the proposed change.
#[tokio::test]
async fn transactions_limit_99999_is_silently_clamped() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "lim_tx_big").await;
    let address = ensure_wallet(&app, &token).await;
    seed_payments(&state.db, &merchant_id, &address, 210).await;

    let (status, json) = send(
        app,
        "GET",
        "/transactions?limit=99999",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "merchant limit=500: {json}");
    let rows = page_items(&json).len();
    assert_eq!(
        rows, 200,
        "merchant limit=500 must clamp to 200 (admin cap is 500): got {rows} rows"
    );
    assert!(
        page_has_more(&json),
        "merchant page clamped to 200 must signal more rows via has_more/next_cursor: {json}"
    );
}

/// #1496 — an admin asking for `limit=500` is allowed the full admin maximum.
#[tokio::test]
async fn admin_limit_allows_up_to_500() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "lim_admin_cap").await;
    let address = ensure_wallet(&app, &token).await;
    seed_payments(&state.db, &merchant_id, &address, 510).await;

    let admin_token = common::ensure_admin(&app, "lim_admin_cap").await;

    let (status, json) = send(
        app.clone(),
        "GET",
        "/admin/transactions?limit=500",
        Some(&admin_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "admin limit=500: {json}");
    let rows = page_items(&json).len();
    assert_eq!(
        rows, 500,
        "admin limit=500 must be honoured (up to 500 rows): got {rows} rows"
    );
}

/// #1496 — `limit=0` and negative limits are treated as the minimum of 1 row.
#[tokio::test]
async fn non_positive_limit_is_treated_as_one() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "lim_nonpos").await;
    let address = ensure_wallet(&app, &token).await;
    seed_payments(&state.db, &merchant_id, &address, 10).await;

    for raw in ["0", "-1", "-500"] {
        let (status, json) = send(
            app.clone(),
            "GET",
            &format!("/transactions?limit={raw}"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "limit={raw}: {json}");
        assert_eq!(
            page_items(&json).len(),
            1,
            "limit={raw} must be treated as the minimum of 1 row"
        );
    }

    // Current behaviour: 200 OK, clamped to the 200-row maximum, no error.
    assert_eq!(
        status,
        StatusCode::OK,
        "/transactions?limit=99999 must currently succeed (clamped): {json}"
    );
    assert_eq!(
        page_items(&json).len(),
        200,
        "/transactions?limit=99999 must currently clamp to 200 rows: {json}"
    );
    assert!(
        json.get("error").is_none(),
        "current behaviour returns no error for out-of-range limit: {json}"
    );
}

/// #1504 — the behaviour *proposed* by
/// docs/proposals/1126-query-limit-error-shape.md: an out-of-range `limit`
/// should be rejected with `400` and the documented JSON error shape
/// `{ "error": "...", "code": "INVALID_PARAMETERS" }` instead of being
/// silently clamped. Ignored until the proposal is implemented.
#[tokio::test]
#[ignore = "pending docs/proposals/1126-query-limit-error-shape.md: out-of-range limit should be 400 INVALID_PARAMETERS, not clamped"]
async fn transactions_limit_99999_should_be_rejected() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "lim_tx_rej").await;
    let address = ensure_wallet(&app, &token).await;
    seed_payments(&state.db, &merchant_id, &address, 210).await;

    let (status, json) = send(
        app,
        "GET",
        "/transactions?limit=99999",
        Some(&token),
        None,
    )
    .await;

    // Proposed behaviour: 400 with the documented error shape.
    assert_bad_limit(status, &json);
}
