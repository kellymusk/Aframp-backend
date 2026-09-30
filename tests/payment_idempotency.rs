mod common;

use aframp::models::NewPayment;
use axum::http::StatusCode;
use serde_json::json;

#[tokio::test]
async fn record_deposit_returns_existing_payment_for_duplicate_tx_hash() {
    let Some(state) = common::state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = common::ensure_merchant(&app, "deposit_idempotency").await;

    let (status, wallet) = common::send(
        app,
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "wallet creation failed: {wallet}");

    let merchant_id: uuid::Uuid = merchant_id.parse().unwrap();
    let wallet_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM wallets WHERE merchant_id = $1",
    )
    .bind(merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    let payment = NewPayment {
        merchant_id,
        wallet_id,
        wallet_address: wallet["address"].as_str().unwrap().to_string(),
        tx_hash: format!("idempotency_{}", uuid::Uuid::new_v4()),
        amount_stroops: 12_345,
        asset: "XLM".to_string(),
        network: "stellar".to_string(),
    };

    let original = aframp::services::payments::record_deposit(&state.db, payment.clone())
        .await
        .expect("first deposit should be recorded");
    let duplicate = aframp::services::payments::record_deposit(&state.db, payment)
        .await
        .expect("duplicate deposit should return the existing payment");

    assert_eq!(duplicate.id, original.id);
    assert_eq!(duplicate.tx_hash, original.tx_hash);
}
