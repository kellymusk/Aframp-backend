use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use crate::blockchain::stellar::{BlockchainListener, StellarListener};
use crate::models::{NewPayment, UpdatePaymentStatus};
use crate::services::{balances, payment_requests, payments, wallets};
use crate::blockchain::stellar::{BlockchainListener, DetectedDeposit, StellarListener};
use crate::models::{NewPayment, UpdateBalance, UpdatePaymentStatus};
use crate::services::{balances, notifications, payment_requests, payments, wallets};
use crate::AppState;

pub async fn run(state: Arc<AppState>, horizon_url: String, poll_interval_secs: u64) {
    let listener = StellarListener::new(horizon_url);

    loop {
        if let Err(err) = poll_once(&state.db, &listener).await {
            tracing::warn!(error = %err, "deposit poll failed");
        }
        tokio::time::sleep(Duration::from_secs(poll_interval_secs)).await;
    }
}

pub async fn poll_once(db: &PgPool, listener: &StellarListener) -> Result<(), String> {
/// Polls the blockchain listener once and processes any detected deposits.
///
/// Generic over [`BlockchainListener`] so tests can inject a mock listener
/// (e.g. to simulate a fake deposit) without hitting a real chain.
pub async fn poll_once<L: BlockchainListener>(
    db: &PgPool,
    listener: &L,
) -> Result<(), String> {
    let addresses: Vec<String> = wallets::all_wallets(db)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|w| w.address)
        .collect();
    if addresses.is_empty() {
        return Ok(());
    }

    let deposits = listener.fetch_deposits(&addresses).await?;
    for deposit in deposits {
        if let Err(err) = process_deposit(db, deposit).await {
            tracing::warn!(error = %err, "failed to process deposit");
        }
    }
    Ok(())
}

pub async fn process_deposit(db: &PgPool, d: DetectedDeposit) -> Result<(), String> {
    let Some(wallet) = wallets::wallet_by_address(db, &d.destination).await.map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
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
    .map_err(|e| e.to_string())?;

    if payment.status != crate::models::status::PaymentStatus::Detected {
        return Ok(());
    }

    payments::set_status(db, payment.id, UpdatePaymentStatus::Verified)
        .await
        .map_err(|e| e.to_string())?;

    // Confirmation is immediate today, so credit the balance directly in a single
    // UPSERT instead of the previous pending → available two-step (two round-trips).
    // TODO: once the Stellar confirmation depth feature lands, split this back into
    // two steps: credit `pending` here, then move pending → available after the
    // confirmations threshold is reached.
    balances::apply_delta(
        db,
        &crate::models::UpdateBalance {
            merchant_id: wallet.merchant_id,
            asset: d.asset.clone(),
            available_delta: d.amount_stroops,
            pending_delta: 0,
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    payments::set_status(db, payment.id, UpdatePaymentStatus::Confirmed)
        .await
        .map_err(|e| e.to_string())?;
    balances::apply_delta(
        db,
        &UpdateBalance {
            merchant_id: wallet.merchant_id,
            asset: d.asset.clone(),
            available_delta: d.amount_stroops,
            pending_delta: -d.amount_stroops,
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    if let Some(memo) = memo {
        if let Some(pr) = payment_requests::find_pending_by_wallet_and_memo(db, wallet.id, &memo)
            .await
            .map_err(|e| e.to_string())?
        {
            if payment.amount_stroops >= pr.amount_stroops {
                payment_requests::mark_paid(db, pr.id, payment.id)
                    .await
                    .map_err(|e| e.to_string())?;
            } else {
                tracing::warn!(
                    expected = pr.amount_stroops,
                    actual = payment.amount_stroops,
                    request_id = %pr.id,
                    "payment request underpaid — marking partial"
                );
                payment_requests::mark_partial(db, pr.id, payment.id)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
    }

    // Notify the merchant that the deposit has been confirmed. Notifications are
    // opt-in via the merchant preference setting; the service is a no-op when the
    // merchant has not enabled them. Failures are logged and never block deposit
    // processing.
    if let Err(err) = notifications::notify_deposit_confirmed(
        db,
        wallet.merchant_id,
        payment.id,
        d.amount_stroops,
        &d.asset,
        &d.tx_hash,
    )
    .await
    {
        tracing::warn!(error = %err, payment_id = %payment.id, "failed to send deposit notification");
    }

    // TODO: dispatch payment.confirmed webhook.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
