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
    let app_state = state().await;
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

    let payment_rows = payments::payments_by_merchant(&app_state.db, merchant_id, 50, None)
        .await
        .expect("failed to load payments");
    assert_eq!(payment_rows.len(), 1, "expected exactly one payment row");
    let payment = &payment_rows[0];
    assert_eq!(payment.wallet_id, wallet.id);
    assert_eq!(payment.amount_stroops, 50_000_000);
    assert_eq!(payment.asset, "XLM");
    assert_eq!(payment.status.as_str(), "confirmed");

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
    let app_state = state().await;
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

    let payment_rows = payments::payments_by_merchant(&app_state.db, merchant_id, 50, None)
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

/// A deposit to an address that isn't one of our wallets is skipped: no
/// payment row, no balance credited anywhere.
#[tokio::test]
async fn poll_once_skips_deposit_to_unknown_address() {
    let app_state = state().await;
    let tx_hash = format!("fake-tx-unknown-{}", Uuid::new_v4());
    let listener = FakeListener {
        deposits: vec![DetectedDeposit {
            tx_hash: tx_hash.clone(),
            destination: format!("GUNKNOWN{}", Uuid::new_v4().simple().to_string().to_uppercase()),
            amount_stroops: 10_000_000,
            asset: "XLM".to_string(),
            confirmations: 1,
            memo: None,
        }],
    };

    poll_once(&app_state.db, &listener).await.expect("poll_once failed");

    let recorded: i64 = sqlx::query_scalar("SELECT count(*) FROM payments WHERE tx_hash = $1")
        .bind(&tx_hash)
        .fetch_one(&app_state.db)
        .await
        .unwrap();
    assert_eq!(recorded, 0, "a deposit to an unknown address must not be recorded");
}

/// The worker exits promptly once shutdown is signalled, both when it was
/// already set and while it sleeps between polls.
#[tokio::test]
async fn worker_stops_on_shutdown_signal() {
    let state = std::sync::Arc::new(state().await);
    // An unreachable Horizon makes each poll fail fast instead of hitting the network.
    let horizon = "http://127.0.0.1:9".to_string();

    let (tx, rx) = tokio::sync::watch::channel(true);
    let already = tokio::spawn(aframp::blockchain::worker::run(state.clone(), horizon.clone(), 3600, 1, 1, rx));
    tokio::time::timeout(std::time::Duration::from_secs(5), already)
        .await
        .expect("worker must not start when shutdown is already set")
        .unwrap();
    drop(tx);

    let (tx, rx) = tokio::sync::watch::channel(false);
    let sleeping = tokio::spawn(aframp::blockchain::worker::run(state, horizon, 3600, 1, 1, rx));
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    tx.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), sleeping)
        .await
        .expect("worker must stop mid-sleep once shutdown is signalled")
        .unwrap();
}

/// An oversized deposit is still credited but flagged for review; a normal
/// one is not.
#[tokio::test]
async fn oversized_deposit_is_credited_and_flagged() {
    let app_state = state().await;
    let router = aframp::router(app_state.clone());
    let (_, merchant_id) = ensure_merchant(&router, "deposit-anomaly").await;
    let merchant_id: Uuid = merchant_id.parse().unwrap();
    let wallet = wallets::create_wallet(&app_state.db, merchant_id, "stellar", &app_state.wallet_encryption_key)
        .await
        .unwrap();

    let deposit = |tx: &str, amount: i64| DetectedDeposit {
        tx_hash: tx.to_string(),
        destination: wallet.address.clone(),
        amount_stroops: amount,
        asset: "XLM".to_string(),
        confirmations: 1,
        memo: None,
    };
    let small = format!("anomaly-small-{}", Uuid::new_v4());
    let large = format!("anomaly-large-{}", Uuid::new_v4());
    // Default single-deposit threshold is 100_000_000_000 stroops.
    let listener = FakeListener { deposits: vec![deposit(&small, 10_000_000), deposit(&large, 100_000_000_001)] };
    poll_once(&app_state.db, &listener).await.expect("poll_once failed");

    let flagged = |tx: String| {
        let db = app_state.db.clone();
        async move {
            sqlx::query_scalar::<_, bool>("SELECT flagged FROM payments WHERE tx_hash = $1")
                .bind(tx)
                .fetch_one(&db)
                .await
                .unwrap()
        }
    };
    assert!(!flagged(small).await, "a normal deposit must not be flagged");
    assert!(flagged(large).await, "an oversized deposit must be flagged");

    let balance = balances::get_balances(&app_state.db, merchant_id).await.unwrap();
    let xlm = balance.iter().find(|b| b.asset == "XLM").unwrap();
    assert_eq!(xlm.available, 100_000_000_001 + 10_000_000, "flagged deposits are still credited");
}
