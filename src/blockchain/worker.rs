use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use crate::blockchain::stellar::{BlockchainListener, StellarListener};
use crate::models::{NewPayment, UpdateBalance, UpdatePaymentStatus};
use crate::services::{balances, payment_requests, payments, wallets};
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

async fn process_deposit(db: &PgPool, d: crate::blockchain::stellar::DetectedDeposit) -> Result<(), String> {
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

    if payment.status != "detected" {
        return Ok(());
    }

    payments::set_status(db, payment.id, UpdatePaymentStatus::Verified)
        .await
        .map_err(|e| e.to_string())?;

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
    .map_err(|e| e.to_string())?;

    // TODO: move pending → available after Stellar confirmations threshold.
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

    // Sweep the confirmed merchant funds to the consolidated settlement wallet.
    if let Err(err) = sweep_confirmed_payment(db, &wallet, &payment, &d.asset).await {
        tracing::warn!(error = %err, payment_id = %payment.id, "platform sweep failed");
    }

    // TODO: dispatch payment.confirmed webhook.
    Ok(())
}

#[cfg(test)]
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
    }
}
