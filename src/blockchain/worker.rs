use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tracing::Instrument;

use crate::blockchain::stellar::{
    AddressCursor, BlockchainListener, DetectedDeposit, StellarListener,
};
use crate::models::{NewPayment, UpdateBalance, UpdatePaymentStatus};
use crate::services::{balances, merchant_webhooks, payment_requests, payments, wallets};
use crate::AppState;

pub async fn run(
    state: Arc<AppState>,
    horizon_url: String,
    poll_interval_secs: u64,
    poll_concurrency: usize,
    min_confirmations: i32,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let listener = StellarListener::new(horizon_url).with_concurrency(poll_concurrency);

    // A poll in progress always runs to completion; shutdown is only observed
    // between polls, so a deposit is never left half-processed.
    while !*shutdown.borrow() {
        let started = std::time::Instant::now();
        let span = tracing::info_span!(
            "stellar_deposit_poll",
            poll_concurrency,
            poll_cycle_duration_ms = tracing::field::Empty
        );
        let result = poll_once_with_confirmations(
            &state.db,
            &listener,
            min_confirmations,
            Some(state.webhook_secret.as_str()),
        )
        .instrument(span.clone())
        .await;
        span.record(
            "poll_cycle_duration_ms",
            started.elapsed().as_millis() as u64,
        );
        if let Err(err) = result {
            tracing::warn!(error = %err, "deposit poll failed");
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(poll_interval_secs)) => {}
            _ = shutdown.changed() => {}
        }
    }
    tracing::info!("deposit worker stopped");
}

/// Polls the blockchain listener once and processes any detected deposits,
/// crediting them after a single confirmation (see
/// [`poll_once_with_confirmations`]).
///
/// Generic over [`BlockchainListener`] so tests can inject a mock listener
/// (e.g. to simulate a fake deposit) without hitting a real chain.
pub async fn poll_once<L: BlockchainListener + ?Sized>(
    db: &PgPool,
    listener: &L,
) -> Result<(), String> {
    poll_once_with_confirmations(db, listener, 1, None).await
}

/// One poll pass: record newly detected deposits (credited to `pending`),
/// then promote deposits that have waited `min_confirmations` passes to
/// `available` (`STELLAR_MIN_CONFIRMATIONS`).
#[tracing::instrument(skip_all, err)]
pub async fn poll_once_with_confirmations<L: BlockchainListener + ?Sized>(
    db: &PgPool,
    listener: &L,
    min_confirmations: i32,
    webhook_secret: Option<&str>,
) -> Result<(), String> {
    // Deposits already waiting gain a confirmation for this pass; new ones
    // recorded below start at one.
    payments::age_verified_deposits(db)
        .await
        .map_err(|e| e.to_string())?;

    let skip = listener.skipped_addresses();
    let wallet_list = wallets::pollable_wallets(db, &skip)
        .await
        .map_err(|e| e.to_string())?;
    if wallet_list.is_empty() {
        return Ok(());
    }

    let address_cursors: Vec<AddressCursor> = wallet_list
        .iter()
        .map(|w| AddressCursor {
            address: w.address.clone(),
            cursor: w.last_polled_cursor.clone(),
        })
        .collect();

    let results = listener.fetch_deposits_since(&address_cursors).await?;
    for result in results {
        let mut all_processed = true;
        for deposit in result.deposits {
            if let Err(err) = process_deposit(db, deposit).await {
                all_processed = false;
                tracing::warn!(error = %err, "failed to process deposit");
            }
        }

        // Only advance the cursor once every deposit in the page was handled,
        // so a transient failure is retried on the next poll rather than
        // skipped forever. Re-processing is safe: deposits dedupe on tx hash.
        let Some(next_cursor) = result.next_cursor else {
            continue;
        };
        let Some(wallet) = wallet_list.iter().find(|w| w.address == result.address) else {
            continue;
        };
        if !all_processed || wallet.last_polled_cursor.as_deref() == Some(next_cursor.as_str()) {
            continue;
        }
        if let Err(err) = wallets::update_last_polled_cursor(db, wallet.id, &next_cursor).await {
            tracing::warn!(error = %err, address = %result.address, "failed to persist poll cursor");
        }
    }
    promote_and_notify(db, min_confirmations, webhook_secret).await
}

/// Second pass: every verified deposit gains one confirmation per poll pass,
/// and those with at least `min_confirmations` move from `pending` to
/// `available` and become `confirmed`. Counting per pass (rather than per
/// re-sighting) is what makes this work with cursor-based polling, which
/// only ever fetches a given operation once.
pub async fn promote_confirmed_deposits(db: &PgPool, min_confirmations: i32) -> Result<(), String> {
    promote_and_notify(db, min_confirmations, None).await
}

/// [`promote_confirmed_deposits`], then queue a `payment.confirmed` webhook
/// to the merchant for each newly confirmed deposit when `webhook_secret` is
/// given. Delivery (with retries) runs in the background and never fails
/// deposit processing.
async fn promote_and_notify(
    db: &PgPool,
    min_confirmations: i32,
    webhook_secret: Option<&str>,
) -> Result<(), String> {
    let promoted = payments::promote_confirmed(db, min_confirmations)
        .await
        .map_err(|e| e.to_string())?;
    if !promoted.is_empty() {
        tracing::info!(
            promoted = promoted.len(),
            min_confirmations,
            "deposits confirmed"
        );
    }
    let Some(secret) = webhook_secret else {
        return Ok(());
    };
    for payment in &promoted {
        if let Err(err) = merchant_webhooks::dispatch(
            db,
            webhook_http_client(),
            secret,
            payment.merchant_id,
            merchant_webhooks::PAYMENT_CONFIRMED,
            &merchant_webhooks::payment_confirmed_payload(payment),
            merchant_webhooks::RetryPolicy::default(),
        )
        .await
        {
            tracing::warn!(error = %err, payment_id = %payment.id, "failed to queue payment.confirmed webhooks");
        }
    }
    Ok(())
}

fn webhook_http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(merchant_webhooks::delivery_client)
}

/// Sizes above which a deposit is flagged for review (it is still credited).
/// Raw stroops, compared per asset.
#[derive(Debug, Clone, Copy)]
pub struct DepositAnomalyThresholds {
    pub max_single_stroops: i64,
    pub max_hourly_stroops: i64,
}

impl Default for DepositAnomalyThresholds {
    fn default() -> Self {
        Self {
            max_single_stroops: 100_000_000_000, // 10,000 units
            max_hourly_stroops: 500_000_000_000, // 50,000 units
        }
    }
}

static ANOMALY_THRESHOLDS: std::sync::OnceLock<DepositAnomalyThresholds> =
    std::sync::OnceLock::new();

/// Sets the thresholds from config at startup; later calls are ignored.
pub fn set_anomaly_thresholds(thresholds: DepositAnomalyThresholds) {
    let _ = ANOMALY_THRESHOLDS.set(thresholds);
}

fn anomaly_thresholds() -> DepositAnomalyThresholds {
    ANOMALY_THRESHOLDS.get().copied().unwrap_or_default()
}

/// Flags (and logs at warn) a freshly recorded deposit that is unusually
/// large on its own or pushes the merchant's last-hour total over the limit.
async fn flag_if_anomalous(
    db: &PgPool,
    payment_id: uuid::Uuid,
    merchant_id: uuid::Uuid,
    d: &DetectedDeposit,
) -> Result<(), String> {
    let limits = anomaly_thresholds();
    let hourly = payments::deposits_in_last_hour(db, merchant_id, &d.asset)
        .await
        .map_err(|e| e.to_string())?;
    let single = d.amount_stroops > limits.max_single_stroops;
    let burst = hourly > limits.max_hourly_stroops;
    if single || burst {
        tracing::warn!(
            %merchant_id,
            tx_hash = %d.tx_hash,
            amount_stroops = d.amount_stroops,
            hourly_total_stroops = hourly,
            single,
            burst,
            "SECURITY: anomalous deposit flagged for review"
        );
        payments::flag_payment(db, payment_id)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tracing::instrument(skip_all, err, fields(tx_hash = %d.tx_hash, destination = %d.destination, amount_stroops = d.amount_stroops, asset = %d.asset))]
pub async fn process_deposit(db: &PgPool, d: DetectedDeposit) -> Result<(), String> {
    let Some(wallet) = wallets::wallet_by_address(db, &d.destination)
        .await
        .map_err(|e| e.to_string())?
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
    flag_if_anomalous(db, payment.id, wallet.merchant_id, &d).await?;

    payments::set_status(db, payment.id, UpdatePaymentStatus::Verified)
        .await
        .map_err(|e| e.to_string())?;
    payments::bump_confirmations(db, payment.id)
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

    if let Some(memo) = memo {
        if let Some(pr) = payment_requests::find_pending_by_wallet_and_memo(db, wallet.id, &memo)
            .await
            .map_err(|e| e.to_string())?
        {
            if !same_asset(&payment.asset, &pr.asset) {
                // The memo matches but the money doesn't: e.g. XLM sent against
                // a cNGN request. The deposit is still credited to the merchant
                // in the asset that arrived, but the request stays unpaid.
                tracing::warn!(
                    request_id = %pr.id,
                    expected_asset = %pr.asset,
                    actual_asset = %payment.asset,
                    "deposit memo matches a payment request in a different asset; not correlating"
                );
            } else if payment.amount_stroops >= pr.amount_stroops {
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

    Ok(())
}

/// Requests may name native lumens as `XLM` or `native`; deposits use `XLM`.
fn same_asset(deposit: &str, requested: &str) -> bool {
    let normalize = |a: &str| {
        if a.eq_ignore_ascii_case("native") {
            "XLM".to_string()
        } else {
            a.to_string()
        }
    };
    normalize(deposit) == normalize(requested)
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

        println!("poll_once with {MERCHANT_COUNT} merchants took {elapsed:?} (budget {budget:?})");
        assert!(
            elapsed <= budget,
            "poll_once took {elapsed:?}, exceeding the {budget:?} budget for {MERCHANT_COUNT} merchants"
        );
    }
}
