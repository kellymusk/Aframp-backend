mod common;

use axum::http::StatusCode;
use serde_json::json;

use aframp::blockchain::stellar::DetectedDeposit;
use aframp::blockchain::worker::{process_deposit, promote_confirmed_deposits};
use common::{ensure_merchant, send, state};

fn deposit(address: &str, tx_hash: &str) -> DetectedDeposit {
    DetectedDeposit {
        tx_hash: tx_hash.to_string(),
        destination: address.to_string(),
        amount_stroops: 50_000_000,
        asset: "XLM".into(),
        confirmations: 1,
        memo: None,
    }
}

#[tokio::test]
async fn deposit_becomes_available_only_after_min_confirmations() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "deposit_confirmations").await;
    let (status, wallet) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{wallet}");
    let address = wallet["address"].as_str().unwrap();
    let tx_hash = format!("tx_{}", uuid::Uuid::new_v4().simple());

    let balance = || async {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT available, pending FROM balances WHERE merchant_id = $1::uuid AND asset = 'XLM'",
        )
        .bind(&merchant_id)
        .fetch_one(&state.db)
        .await
        .unwrap()
    };

    // First pass: detected — pending only, even after the promotion pass.
    process_deposit(&state.db, deposit(address, &tx_hash))
        .await
        .unwrap();
    promote_confirmed_deposits(&state.db, 2).await.unwrap();
    assert_eq!(balance().await, (0, 50_000_000));

    // Next poll pass (cursor polling doesn't re-fetch it; it just ages):
    // now at 2 confirmations and promoted.
    aframp::services::payments::age_verified_deposits(&state.db)
        .await
        .unwrap();
    promote_confirmed_deposits(&state.db, 2).await.unwrap();
    assert_eq!(balance().await, (50_000_000, 0));

    // Further passes, or a re-sighting, don't credit it twice.
    process_deposit(&state.db, deposit(address, &tx_hash))
        .await
        .unwrap();
    aframp::services::payments::age_verified_deposits(&state.db)
        .await
        .unwrap();
    promote_confirmed_deposits(&state.db, 2).await.unwrap();
    assert_eq!(balance().await, (50_000_000, 0));

    let status: String = sqlx::query_scalar("SELECT status FROM payments WHERE tx_hash = $1")
        .bind(&tx_hash)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(status, "confirmed");
}

/// Creates a payment request for `amount_stroops` and returns its id.
async fn create_payment_request(
    app: &axum::Router,
    token: &str,
    amount_stroops: i64,
    memo: &str,
) -> String {
    let (status, body) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(token),
        Some(json!({
            "amount_stroops": amount_stroops,
            "asset": "XLM",
            "memo": memo,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["id"].as_str().unwrap().to_string()
}

async fn payment_request_status(state: &common::TestState, id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM payment_requests WHERE id = $1::uuid")
        .bind(id)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

/// #1502: a deposit whose received amount is strictly less than the payment
/// request amount must mark the request `partial`, never `paid` — regardless
/// of whether the customer used a plain payment or a strict-send path payment.
/// See `docs/proposals/1110-strict-send-amount-received.md`.
#[tokio::test]
#[ignore = "worker still marks underpaid requests paid; see docs/proposals/1110-strict-send-amount-received.md"]
async fn underpaid_deposit_marks_request_partial_not_paid() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _merchant_id) = ensure_merchant(&app, "deposit_underpaid").await;
    let (_, wallet) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    let address = wallet["address"].as_str().unwrap();

    // Plain payment: 50 XLM received against a 100 XLM request.
    let memo = format!("pr_{}", uuid::Uuid::new_v4().simple());
    let request_id = create_payment_request(&app, &token, 100_000_000, &memo).await;
    let mut plain = deposit(address, &format!("tx_{}", uuid::Uuid::new_v4().simple()));
    plain.memo = Some(memo.clone());
    process_deposit(&state.db, plain).await.unwrap();
    assert_eq!(
        payment_request_status(&state, &request_id).await,
        "partial",
        "underpaid plain deposit must not mark the request paid"
    );

    // Path payment (strict-send): same received amount, same outcome.
    let memo = format!("pr_{}", uuid::Uuid::new_v4().simple());
    let request_id = create_payment_request(&app, &token, 100_000_000, &memo).await;
    let mut path = deposit(address, &format!("tx_{}", uuid::Uuid::new_v4().simple()));
    path.memo = Some(memo.clone());
    path.amount_stroops = 50_000_000;
    process_deposit(&state.db, path).await.unwrap();
    assert_eq!(
        payment_request_status(&state, &request_id).await,
        "partial",
        "underpaid path payment must not mark the request paid"
    );
}

/// Companion to the underpayment case: a deposit that meets or exceeds the
/// request amount marks it `paid`, for both plain and path payments.
#[tokio::test]
async fn sufficient_deposit_marks_request_paid() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _merchant_id) = ensure_merchant(&app, "deposit_sufficient").await;
    let (_, wallet) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    let address = wallet["address"].as_str().unwrap();

    // Plain payment: exactly the requested amount.
    let memo = format!("pr_{}", uuid::Uuid::new_v4().simple());
    let request_id = create_payment_request(&app, &token, 50_000_000, &memo).await;
    let mut plain = deposit(address, &format!("tx_{}", uuid::Uuid::new_v4().simple()));
    plain.memo = Some(memo.clone());
    process_deposit(&state.db, plain).await.unwrap();
    assert_eq!(
        payment_request_status(&state, &request_id).await,
        "paid",
        "fully paid plain deposit must mark the request paid"
    );

    // Path payment (strict-send): received amount exceeds the request.
    let memo = format!("pr_{}", uuid::Uuid::new_v4().simple());
    let request_id = create_payment_request(&app, &token, 50_000_000, &memo).await;
    let mut path = deposit(address, &format!("tx_{}", uuid::Uuid::new_v4().simple()));
    path.memo = Some(memo.clone());
    path.amount_stroops = 60_000_000;
    process_deposit(&state.db, path).await.unwrap();
    assert_eq!(
        payment_request_status(&state, &request_id).await,
        "paid",
        "overpaid path payment must mark the request paid"
    );
}

#[tokio::test]
async fn deposit_with_default_one_confirmation_is_available_after_first_pass() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "deposit_default_confirmations").await;
    let (_, wallet) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    let address = wallet["address"].as_str().unwrap();
    let tx_hash = format!("tx_{}", uuid::Uuid::new_v4().simple());

    process_deposit(&state.db, deposit(address, &tx_hash))
        .await
        .unwrap();
    promote_confirmed_deposits(&state.db, 1).await.unwrap();

    let (available, pending): (i64, i64) = sqlx::query_as(
        "SELECT available, pending FROM balances WHERE merchant_id = $1::uuid AND asset = 'XLM'",
    )
    .bind(&merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!((available, pending), (50_000_000, 0));
}
