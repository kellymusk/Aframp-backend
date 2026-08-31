mod common;

use async_trait::async_trait;
use uuid::Uuid;

use aframp::blockchain::stellar::{BlockchainListener, DetectedDeposit};
use aframp::blockchain::worker::poll_once;
use aframp::services::{balances, payments, wallets};

use common::{ensure_merchant, state};

struct FakeListener {
    deposits: Vec<DetectedDeposit>,
}

#[async_trait]
impl BlockchainListener for FakeListener {
    async fn fetch_deposits(&self, _addresses: &[String]) -> Result<Vec<DetectedDeposit>, String> {
        Ok(self.deposits.clone())
    }
}

#[tokio::test]
async fn poll_once_records_payment_and_credits_balance_for_detected_deposit() {
    let Some(app_state) = state().await else {
        return;
    };
    let router = aframp::router(app_state.clone());

    let (_, merchant_id_str) = ensure_merchant(&router, "deposit-worker").await;
    let merchant_id: Uuid = merchant_id_str.parse().unwrap();

    let wallet = wallets::create_wallet(
        &app_state.db,
        merchant_id,
        "stellar",
        &app_state.wallet_encryption_key,
    )
    .await
    .expect("wallet creation failed");

    let listener = FakeListener {
        deposits: vec![DetectedDeposit {
            tx_hash: format!("fake-tx-{}", Uuid::new_v4()),
            destination: wallet.address.clone(),
            amount_stroops: 50_000_000,
            asset: "XLM".to_string(),
            confirmations: 1,
            memo: None,
        }],
    };

    poll_once(&app_state.db, &listener)
        .await
        .expect("poll_once failed");

    let payment_rows = payments::payments_by_merchant(&app_state.db, merchant_id, 50)
        .await
        .expect("failed to load payments");
    assert_eq!(payment_rows.len(), 1, "expected exactly one payment row");
    let payment = &payment_rows[0];
    assert_eq!(payment.wallet_id, wallet.id);
    assert_eq!(payment.amount_stroops, 50_000_000);
    assert_eq!(payment.asset, "XLM");
    assert_eq!(payment.status, "confirmed");

    let balance_rows = balances::get_balances(&app_state.db, merchant_id)
        .await
        .expect("failed to load balances");
    let xlm_balance = balance_rows
        .iter()
        .find(|b| b.asset == "XLM")
        .expect("expected an XLM balance row");
    assert_eq!(xlm_balance.available, 50_000_000);
    assert_eq!(xlm_balance.pending, 0);
}

#[tokio::test]
async fn poll_once_is_idempotent_for_a_repeated_deposit() {
    let Some(app_state) = state().await else {
        return;
    };
    let router = aframp::router(app_state.clone());

    let (_, merchant_id_str) = ensure_merchant(&router, "deposit-worker-dup").await;
    let merchant_id: Uuid = merchant_id_str.parse().unwrap();

    let wallet = wallets::create_wallet(
        &app_state.db,
        merchant_id,
        "stellar",
        &app_state.wallet_encryption_key,
    )
    .await
    .expect("wallet creation failed");

    let tx_hash = format!("fake-tx-{}", Uuid::new_v4());
    let listener = FakeListener {
        deposits: vec![DetectedDeposit {
            tx_hash: tx_hash.clone(),
            destination: wallet.address.clone(),
            amount_stroops: 20_000_000,
            asset: "XLM".to_string(),
            confirmations: 1,
            memo: None,
        }],
    };

    poll_once(&app_state.db, &listener).await.expect("first poll_once failed");
    poll_once(&app_state.db, &listener).await.expect("second poll_once failed");

    let payment_rows = payments::payments_by_merchant(&app_state.db, merchant_id, 50)
        .await
        .expect("failed to load payments");
    assert_eq!(
        payment_rows.len(),
        1,
        "the same tx_hash must not produce a second payment row"
    );

    let balance_rows = balances::get_balances(&app_state.db, merchant_id)
        .await
        .expect("failed to load balances");
    let xlm_balance = balance_rows
        .iter()
        .find(|b| b.asset == "XLM")
        .expect("expected an XLM balance row");
    assert_eq!(
        xlm_balance.available, 20_000_000,
        "re-polling the same deposit must not double-credit the balance"
    );
}
