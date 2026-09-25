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
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "deposit_confirmations").await;
    let (status, wallet) = send(app.clone(), "POST", "/wallet/create", Some(&token), Some(json!({}))).await;
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

    // First pass: detected once — pending only, even after the promotion pass.
    process_deposit(&state.db, deposit(address, &tx_hash)).await.unwrap();
    promote_confirmed_deposits(&state.db, 2).await.unwrap();
    assert_eq!(balance().await, (0, 50_000_000));

    // Seen again on the next poll: now at 2 confirmations and promoted.
    process_deposit(&state.db, deposit(address, &tx_hash)).await.unwrap();
    promote_confirmed_deposits(&state.db, 2).await.unwrap();
    assert_eq!(balance().await, (50_000_000, 0));

    // Further sightings and passes don't credit it twice.
    process_deposit(&state.db, deposit(address, &tx_hash)).await.unwrap();
    promote_confirmed_deposits(&state.db, 2).await.unwrap();
    assert_eq!(balance().await, (50_000_000, 0));

    let status: String = sqlx::query_scalar("SELECT status FROM payments WHERE tx_hash = $1")
        .bind(&tx_hash)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(status, "confirmed");
}

#[tokio::test]
async fn deposit_with_default_one_confirmation_is_available_after_first_pass() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "deposit_default_confirmations").await;
    let (_, wallet) = send(app.clone(), "POST", "/wallet/create", Some(&token), Some(json!({}))).await;
    let address = wallet["address"].as_str().unwrap();
    let tx_hash = format!("tx_{}", uuid::Uuid::new_v4().simple());

    process_deposit(&state.db, deposit(address, &tx_hash)).await.unwrap();
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
