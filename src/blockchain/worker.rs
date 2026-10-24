use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::CancellationToken;
use tokio::time::{interval, MissedTickBehavior};
use tokio::sync::watch;

use crate::blockchain::error::BlockchainError;
use crate::blockchain::stellar::{BlockchainListener, StellarListener};
use crate::models::{NewPayment, UpdateBalance, UpdatePaymentStatus, Wallet};
use crate::services::{balances, payment_requests, payments, wallets};
use crate::AppState;

/// Default per-merchant single-deposit threshold (in stroops) used when the
/// `MAX_SINGLE_DEPOSIT_STROOPS` environment variable is not set.
const DEFAULT_MAX_SINGLE_DEPOSIT_STROOPS: i64 = 100_000_000_000; // 10,000 XLM

/// Default per-merchant hourly aggregate threshold (in stroops) used when the
/// `MAX_HOURLY_DEPOSIT_STROOPS` environment variable is not set.
const DEFAULT_MAX_HOURLY_DEPOSIT_STROOPS: i64 = 500_000_000_000; // 50,000 XLM

fn max_single_deposit_stroops() -> i64 {
    std::env::var("MAX_SINGLE_DEPOSIT_STROOPS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_MAX_SINGLE_DEPOSIT_STROOPS)
}

fn max_hourly_deposit_stroops() -> i64 {
    std::env::var("MAX_HOURLY_DEPOSIT_STROOPS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_MAX_HOURLY_DEPOSIT_STROOPS)
}

pub async fn run(
    state: Arc<AppState>,
    horizon_url: String,
    poll_interval_secs: u64,
    shutdown: CancellationToken,
    mut shutdown: watch::Receiver<bool>,
) {
    let listener = StellarListener::new(horizon_url);

    let mut ticker = interval(Duration::from_secs(poll_interval_secs.max(1)));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!("deposit worker received shutdown signal, exiting");
                break;
            }
            _ = ticker.tick() => {
                if let Err(err) = poll_once(&state.db, &listener).await {
                    tracing::warn!(error = %err, "deposit poll failed");
                }
        if *shutdown.borrow() {
            tracing::info!("blockchain worker received shutdown signal, stopping");
            return;
        }

        if let Err(err) = poll_once(&state.db, &listener).await {
            log_blockchain_error(&err, "deposit poll failed");
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(poll_interval_secs)) => {}
            _ = shutdown.changed() => {
                tracing::info!("blockchain worker received shutdown signal, stopping");
                return;
            }
        }
    }
}

async fn poll_once(db: &PgPool, listener: &StellarListener) -> Result<(), String> {
    let wallets_by_address: HashMap<String, Wallet> = wallets::all_wallets(db)
/// Log a `BlockchainError` at the appropriate severity level:
///
/// - `HttpError`     → `warn`  (transient network issue; next cycle will retry)
/// - `ParseError`    → `error` (bad data from Horizon; retrying won't help)
/// - `DatabaseError` → `error` (platform storage problem; needs attention)
/// - `NotFound`      → `warn`  (can occur transiently during wallet creation)
fn log_blockchain_error(err: &BlockchainError, context: &str) {
    match err {
        BlockchainError::HttpError(_) | BlockchainError::NotFound(_) => {
            tracing::warn!(error = %err, "{context}");
        }
        BlockchainError::ParseError(_) | BlockchainError::DatabaseError(_) => {
            tracing::error!(error = %err, "{context}");
        }
    }
}

async fn poll_once(
    db: &PgPool,
    listener: &StellarListener,
) -> Result<(), BlockchainError> {
    let addresses: Vec<String> = wallets::all_wallets(db)
        .await
        .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?
        .into_iter()
        .map(|w| (w.address.clone(), w))
        .collect();
    if wallets_by_address.is_empty() {
        return Ok(());
    }
    let addresses: Vec<String> = wallets_by_address.keys().cloned().collect();

    let addresses: Vec<String> = wallets_by_address.keys().cloned().collect();
    let deposits = listener.fetch_deposits(&addresses).await?;
    for deposit in deposits {
        if let Err(err) = process_deposit(db, &wallets_by_address, deposit).await {
            tracing::warn!(error = %err, "failed to process deposit");
        if let Err(err) = process_deposit(db, deposit).await {
            log_blockchain_error(&err, "failed to process deposit");
        }
    }
    Ok(())
}

async fn process_deposit(
    db: &PgPool,
    wallets_by_address: &HashMap<String, Wallet>,
    d: crate::blockchain::stellar::DetectedDeposit,
) -> Result<(), String> {
    let Some(wallet) = wallets_by_address.get(&d.destination).cloned() else {
        tracing::warn!(address = %d.destination, "deposit for unknown wallet");
/// Returns true only when the deposit's destination is exactly the address of
/// the wallet that would be credited.
pub fn destination_matches_wallet(wallet_address: &str, destination: &str) -> bool {
    wallet_address == destination
}

pub async fn process_deposit(
    db: &PgPool,
    d: crate::blockchain::stellar::DetectedDeposit,
) -> Result<(), BlockchainError> {
    let Some(wallet) = wallets::wallet_by_address(db, &d.destination)
        .await
        .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?
    else {
        return Ok(());
    };
    // Defence in depth: never credit a wallet whose address doesn't match the
    // deposit destination, even if address → wallet resolution misbehaves.
    if !destination_matches_wallet(&wallet.address, &d.destination) {
        tracing::warn!(
            wallet_id = %wallet.id,
            merchant_id = %wallet.merchant_id,
            wallet_address = %wallet.address,
            destination = %d.destination,
            tx_hash = %d.tx_hash,
            "deposit destination does not match resolved wallet address — skipping"
        );
        return Ok(());
    }
    let memo = d.memo.clone();

    let payment = payments::record_deposit(
        db,
        NewPayment {
            merchant_id: wallet.merchant_id,
            wallet_id: wallet.id,
            wallet_address: wallet.address.clone(),
            tx_hash: d.tx_hash.clone(),
            amount_stroops: d.amount_stroops,
            asset: d.asset.clone(),
            network: "stellar".into(),
        },
    )
    .await
    .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;

    if payment.status != crate::models::status::PaymentStatus::Detected {
    if payment.status != "detected" {
        return Ok(());
    }

    // Anomaly detection: flag unusually large single deposits or hourly bursts.
    // The deposit is still credited below; flagging only raises an alert.
    let mut flagged = false;
    let single_threshold = max_single_deposit_stroops();
    if d.amount_stroops > single_threshold {
        flagged = true;
        tracing::warn!(
            merchant_id = %wallet.merchant_id,
            amount_stroops = d.amount_stroops,
            tx_hash = %d.tx_hash,
            threshold = single_threshold,
            "SECURITY: deposit exceeds MAX_SINGLE_DEPOSIT_STROOPS"
        );
    }

    let hourly_threshold = max_hourly_deposit_stroops();
    match payments::sum_recent_deposits_stroops(db, wallet.merchant_id, 3600).await {
        Ok(recent_total) => {
            let projected = recent_total.saturating_add(d.amount_stroops);
            if projected > hourly_threshold {
                flagged = true;
                tracing::warn!(
                    merchant_id = %wallet.merchant_id,
                    amount_stroops = d.amount_stroops,
                    hourly_total_stroops = projected,
                    tx_hash = %d.tx_hash,
                    threshold = hourly_threshold,
                    "SECURITY: deposit exceeds MAX_HOURLY_DEPOSIT_STROOPS"
                );
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to compute hourly deposit total");
        }
    }

    if flagged {
        payments::flag_payment(db, payment.id)
            .await
            .map_err(|e| e.to_string())?;
    }

    payments::set_status(db, payment.id, UpdatePaymentStatus::Verified)
        .await
        .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;

    balances::apply_delta(
        db,
        &UpdateBalance {
            merchant_id: wallet.merchant_id,
            asset: d.asset.clone(),
            available_delta: 0,
            pending_delta: d.amount_stroops,
        },
    )
    .await
    .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;

    // TODO: move pending → available after Stellar confirmations threshold.
    payments::set_status(db, payment.id, UpdatePaymentStatus::Confirmed)
        .await
        .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;
    balances::apply_delta(
        db,
        &UpdateBalance {
            merchant_id: wallet.merchant_id,
            asset: d.asset,
            available_delta: d.amount_stroops,
            pending_delta: -d.amount_stroops,
        },
    )
    .await
    .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;

    if let Some(memo) = memo {
        if let Some(pr) = payment_requests::find_pending_by_wallet_and_memo(db, wallet.id, &memo)
            .await
            .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?
        {
            if payment.amount_stroops >= pr.amount_stroops {
                payment_requests::mark_paid(db, pr.id, payment.id)
                    .await
                    .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;
            } else {
                tracing::warn!(
                    expected = pr.amount_stroops,
                    actual = payment.amount_stroops,
                    request_id = %pr.id,
                    "payment request underpaid — marking partial"
                );
                payment_requests::mark_partial(db, pr.id, payment.id)
                    .await
                    .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;
            }
        }
    }

    // TODO: dispatch payment.confirmed webhook.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_exits_when_cancelled() {
        let shutdown = CancellationToken::new();
        shutdown.cancel();

        // A cancelled token must cause `run` to return promptly rather than
        // looping forever. We use a long poll interval so the only way the
        // future can complete is via the cancellation branch.
        let handle = tokio::time::timeout(
            Duration::from_secs(5),
            run_without_state(shutdown.clone(), 3600),
        )
        .await;

        assert!(handle.is_ok(), "worker did not exit after cancellation");
    }

    /// Mirrors the select loop in `run` without requiring a live `AppState`/DB,
    /// so the cancellation path can be exercised in isolation.
    async fn run_without_state(shutdown: CancellationToken, poll_interval_secs: u64) {
        let mut ticker = interval(Duration::from_secs(poll_interval_secs.max(1)));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = ticker.tick() => {}
            }
        }
    }
mod destination_check_tests {
    use super::destination_matches_wallet;

    #[test]
    fn matching_destination_is_accepted() {
        let addr = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
        assert!(destination_matches_wallet(addr, addr));
    }

    #[test]
    fn mismatched_destination_is_rejected() {
        assert!(!destination_matches_wallet(
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "GBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBWHF",
        ));
        assert!(!destination_matches_wallet("GABC", "gabc"));
        assert!(!destination_matches_wallet("GABC", ""));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::NewPayment;
    use crate::services::{balances, payments, wallets};

    /// A deposit whose destination doesn't resolve to a known wallet must be
    /// skipped without recording a payment or crediting any merchant.
    #[sqlx::test]
    async fn deposit_to_unknown_destination_is_skipped(db: PgPool) {
        let merchant_id = uuid::Uuid::new_v4();
        wallets::create(
            &db,
            crate::models::NewWallet {
                merchant_id,
                address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
                network: "stellar".into(),
            },
        )
        .await
        .expect("create wallet");

        let deposit = crate::blockchain::stellar::DetectedDeposit {
            tx_hash: "mismatch".into(),
            destination: "GBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBWHF".into(),
            amount_stroops: 1_000,
            asset: "XLM".into(),
            memo: None,
        };
        process_deposit(&db, deposit).await.expect("process_deposit");

        let rows = payments::payments_by_merchant(&db, merchant_id, 10)
            .await
            .expect("list payments");
        assert!(rows.is_empty(), "mismatched destination must not be credited");
    }

    /// Simulates a crash between `record_deposit` (which inserts a row with
    /// status `detected`) and `set_status(Verified)`. On the next poll cycle
    /// `record_deposit` returns the existing row; `process_deposit` must NOT
    /// credit the pending balance a second time.
    #[sqlx::test]
    async fn deposit_reentrancy_does_not_double_credit(db: PgPool) {
        // Arrange: a wallet for the merchant.
        let merchant_id = uuid::Uuid::new_v4();
        let wallet = wallets::create(
            &db,
            crate::models::NewWallet {
                merchant_id,
                address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
                network: "stellar".into(),
            },
        )
        .await
        .expect("create wallet");

        let deposit = crate::blockchain::stellar::DetectedDeposit {
            tx_hash: "deadbeef".into(),
            destination: wallet.address.clone(),
            amount_stroops: 1_000,
            asset: "XLM".into(),
            memo: None,
        };

        // First poll: fresh insert, credits pending once.
        process_deposit(&db, deposit.clone())
            .await
            .expect("first process_deposit");

        // Simulate the crash: force the row back to `detected` so the
        // status-based guard does not fire, mimicking a partial write.
        let existing = payments::record_deposit(
            &db,
            NewPayment {
                merchant_id,
                wallet_id: wallet.id,
                wallet_address: wallet.address.clone(),
                tx_hash: deposit.tx_hash.clone(),
                amount_stroops: deposit.amount_stroops,
                asset: deposit.asset.clone(),
                network: "stellar".into(),
            },
        )
        .await
        .expect("re-record deposit");
        payments::set_status(&db, existing.0.id, crate::models::UpdatePaymentStatus::Detected)
            .await
            .expect("reset to detected");

        // Second poll with the same tx_hash: must be a no-op for balances.
        process_deposit(&db, deposit.clone())
            .await
            .expect("second process_deposit");

        let balance = balances::get(&db, merchant_id, "XLM")
            .await
            .expect("get balance");
        assert_eq!(
            balance.pending_stroops, 0,
            "pending balance must be credited exactly once (already moved to available)"
        );
        assert_eq!(
            balance.available_stroops, deposit.amount_stroops,
            "available balance must be credited exactly once"
        );
    use crate::models::UpdateBalance;
    use crate::services::{balances, payments, wallets};
    use sqlx::PgPool;

    /// Verifies that a confirmed deposit credits the merchant's available balance
    /// exactly once (single UPSERT) and leaves pending untouched.
    #[sqlx::test]
    async fn deposit_credits_available_balance_once(db: PgPool) {
        let merchant_id = uuid::Uuid::new_v4();
        let wallet = wallets::create(
            &db,
            merchant_id,
            "stellar",
            "GTESTDEPOSITWALLETADDRESS000000000000000000000000000000000",
        )
        .await
        .expect("wallet created");

        let amount: i64 = 1_000_000;

        // Mirrors the single UPSERT performed by process_deposit for an immediate confirmation.
        balances::apply_delta(
            &db,
            &UpdateBalance {
                merchant_id,
                asset: "XLM".into(),
                available_delta: amount,
                pending_delta: 0,
            },
        )
        .await
        .expect("balance credited");

        let balance = balances::get(&db, merchant_id, "XLM")
            .await
            .expect("balance fetched");

        assert_eq!(balance.available, amount);
        assert_eq!(balance.pending, 0);

        // A second deposit should accumulate, not overwrite.
        balances::apply_delta(
            &db,
            &UpdateBalance {
                merchant_id,
                asset: "XLM".into(),
                available_delta: amount,
                pending_delta: 0,
            },
        )
        .await
        .expect("balance credited again");

        let balance = balances::get(&db, merchant_id, "XLM")
            .await
            .expect("balance fetched");
        assert_eq!(balance.available, amount * 2);
        assert_eq!(balance.pending, 0);

        // Sanity: the wallet lookup used by process_deposit still resolves.
        let found = wallets::wallet_by_address(&db, &wallet.address)
            .await
            .expect("wallet lookup")
            .expect("wallet present");
        assert_eq!(found.merchant_id, merchant_id);

        // Keep the payments import meaningful for the test module.
        let _ = payments::record_deposit;
mod load_tests {
    use super::*;
    use crate::blockchain::stellar::StellarListener;
    use sqlx::PgPool;
    use std::time::Instant;

    /// Number of merchants/wallets registered for the load test.
    const MERCHANT_COUNT: usize = 100;

    /// Performance regression threshold: a single poll cycle must complete
    /// within this multiple of the configured poll interval.
    const POLL_CYCLE_BUDGET_MULTIPLIER: u64 = 2;

    /// Registers `count` merchants, each with a single wallet, and returns the
    /// generated wallet addresses.
    async fn seed_merchants_and_wallets(db: &PgPool, count: usize) -> Vec<String> {
        let mut addresses = Vec::with_capacity(count);
        for i in 0..count {
            let merchant_id = sqlx::query_scalar::<_, uuid::Uuid>(
                "INSERT INTO merchants (name, email) VALUES ($1, $2) RETURNING id",
            )
            .bind(format!("load-merchant-{i}"))
            .bind(format!("load-merchant-{i}@example.test"))
            .fetch_one(db)
            .await
            .expect("insert merchant");

            let address = format!("GLOAD{i:055}");
            sqlx::query(
                "INSERT INTO wallets (merchant_id, address, network) VALUES ($1, $2, 'stellar')",
            )
            .bind(merchant_id)
            .bind(&address)
            .execute(db)
            .await
            .expect("insert wallet");

            addresses.push(address);
        }
        addresses
    }

    /// Load test: with 100+ merchants registered, a single `poll_once` cycle
    /// must complete within `2 * poll_interval_secs`.
    ///
    /// Requires a test database (`DATABASE_URL`) and a mock Horizon server
    /// returning empty responses. Run with:
    /// `cargo test -- --ignored deposit_worker_handles_100_concurrent_merchants`
    #[tokio::test]
    #[ignore = "requires DATABASE_URL and a mock Horizon server"]
    async fn deposit_worker_handles_100_concurrent_merchants() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let db = PgPool::connect(&database_url)
            .await
            .expect("connect test db");

        let addresses = seed_merchants_and_wallets(&db, MERCHANT_COUNT).await;
        assert_eq!(addresses.len(), MERCHANT_COUNT);

        // Mock Horizon server returning empty responses for every account.
        let horizon_url = std::env::var("MOCK_HORIZON_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8089".to_string());
        let listener = StellarListener::new(horizon_url);

        let poll_interval_secs: u64 = std::env::var("STELLAR_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5);
        let budget = Duration::from_secs(poll_interval_secs * POLL_CYCLE_BUDGET_MULTIPLIER);

        let start = Instant::now();
        poll_once(&db, &listener).await.expect("poll_once cycle");
        let elapsed = start.elapsed();

        println!(
            "poll_once with {MERCHANT_COUNT} merchants took {elapsed:?} (budget {budget:?})"
        );
        assert!(
            elapsed <= budget,
            "poll_once took {elapsed:?}, exceeding the {budget:?} budget for {MERCHANT_COUNT} merchants"
        );
mod tests {
    use super::*;
    use crate::blockchain::stellar::DetectedDeposit;
    use crate::otp::mock::MockOtpProvider;
    use crate::otp::OtpProvider;
    use crate::services::{balances, payment_requests, payments, wallets};
    use crate::models::{NewPaymentRequest, NewWallet};

    /// A mock [`BlockchainListener`] that returns a pre-seeded set of deposits
    /// instead of querying a real Horizon node.
    struct MockBlockchainListener {
        deposits: Vec<DetectedDeposit>,
    }

    #[async_trait::async_trait]
    impl BlockchainListener for MockBlockchainListener {
        async fn fetch_deposits(
            &self,
            _addresses: &[String],
        ) -> Result<Vec<DetectedDeposit>, String> {
            Ok(self.deposits.clone())
        }
    }

    /// End-to-end integration test for the full merchant onboarding flow:
    /// signup -> OTP verify -> create wallet -> create payment request ->
    /// detect deposit -> check balance.
    #[sqlx::test]
    async fn merchant_onboarding_end_to_end(db: PgPool) {
        // 1. Signup: create the merchant record.
        let merchant = sqlx::query!(
            r#"
            INSERT INTO merchants (name, email, password_hash)
            VALUES ($1, $2, $3)
            RETURNING id
            "#,
            "Test Merchant",
            "merchant@example.com",
            "hashed-password",
        )
        .fetch_one(&db)
        .await
        .expect("merchant signup should succeed");

        // 2. OTP verify: use MockOtpProvider to skip real SMS.
        let otp = MockOtpProvider::new();
        let code = otp
            .send_code("merchant@example.com")
            .await
            .expect("mock OTP send should succeed");
        let verified = otp
            .verify_code("merchant@example.com", &code)
            .await
            .expect("mock OTP verify should succeed");
        assert!(verified, "OTP verification should succeed");

        // 3. Create wallet for the merchant.
        let wallet = wallets::create(
            &db,
            NewWallet {
                merchant_id: merchant.id,
                address: "GTESTWALLETADDRESS0000000000000000000000000000000000000000".into(),
                network: "stellar".into(),
            },
        )
        .await
        .expect("wallet creation should succeed");

        // 4. Create a payment request tied to the wallet via memo.
        let memo = "onboarding-memo-1";
        let amount_stroops: i64 = 10_000_000;
        let payment_request = payment_requests::create(
            &db,
            NewPaymentRequest {
                merchant_id: merchant.id,
                wallet_id: wallet.id,
                memo: memo.into(),
                amount_stroops,
                asset: "XLM".into(),
            },
        )
        .await
        .expect("payment request creation should succeed");

        // 5. Detect deposit: inject a fake deposit through the mock listener.
        let listener = MockBlockchainListener {
            deposits: vec![DetectedDeposit {
                tx_hash: "fake-tx-hash-1".into(),
                destination: wallet.address.clone(),
                amount_stroops,
                asset: "XLM".into(),
                memo: Some(memo.into()),
            }],
        };
        poll_once(&db, &listener)
            .await
            .expect("deposit polling should succeed");

        // 6. Check balance equals the injected deposit amount.
        let balance = balances::get(&db, merchant.id, "XLM")
            .await
            .expect("balance lookup should succeed");
        assert_eq!(
            balance.available_stroops, amount_stroops,
            "available balance should equal the deposit amount"
        );

        // 7. Assert the payment request status is 'paid'.
        let updated = payment_requests::get(&db, payment_request.id)
            .await
            .expect("payment request lookup should succeed");
        assert_eq!(updated.status, "paid", "payment request should be paid");
    }
}
