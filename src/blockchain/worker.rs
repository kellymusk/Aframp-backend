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
    let single = d.amount_stroops;
    let mut reasons: Vec<&str> = Vec::new();
    if single > limits.max_single_stroops {
        reasons.push("single_deposit_exceeds_threshold");
    }
    if hourly > limits.max_hourly_stroops {
        reasons.push("hourly_volume_exceeds_threshold");
    }
    if reasons.is_empty() {
        return Ok(());
    }
    tracing::warn!(
        payment_id = %payment_id,
        asset = %d.asset,
        amount_stroops = single,
        hourly_stroops = hourly,
        reasons = ?reasons,
        "deposit flagged as anomalous"
    );
    payments::flag_anomaly(db, payment_id, &reasons)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Normalises an asset code for comparison: the native asset is reported by
/// Horizon as `XLM` but may be stored as `native` (or vice versa), so both
/// spellings collapse to a single canonical form.
fn normalise_asset(asset: &str) -> &str {
    if asset.eq_ignore_ascii_case("native") {
        "XLM"
    } else {
        asset
    }
}

/// Returns true when two asset codes refer to the same asset, treating
/// `native` and `XLM` as equivalent.
fn same_asset(a: &str, b: &str) -> bool {
    normalise_asset(a).eq_ignore_ascii_case(normalise_asset(b))
}

/// Processes a single detected deposit: records it, credits the wallet, and
/// settles any matching pending payment request.
async fn process_deposit(db: &PgPool, d: DetectedDeposit) -> Result<(), String> {
    let wallet = wallets::find_by_address(db, &d.address)
        .await
        .map_err(|e| e.to_string())?;
    let Some(wallet) = wallet else {
        tracing::warn!(address = %d.address, "deposit for unknown wallet");
        return Ok(());
    };

    let new_payment = NewPayment {
        merchant_id: wallet.merchant_id,
        wallet_id: wallet.id,
        tx_hash: d.tx_hash.clone(),
        from_address: d.from_address.clone(),
        asset: d.asset.clone(),
        amount_stroops: d.amount_stroops,
        memo: d.memo.clone(),
    };
    let payment = match payments::insert_deposit(db, &new_payment).await {
        Ok(p) => p,
        Err(err) => {
            // Duplicate tx hash: already processed on an earlier poll.
            tracing::debug!(error = %err, tx_hash = %d.tx_hash, "deposit already recorded");
            return Ok(());
        }
    };

    let update = UpdateBalance {
        wallet_id: wallet.id,
        asset: d.asset.clone(),
        delta_stroops: d.amount_stroops,
    };
    balances::apply_delta(db, &update)
        .await
        .map_err(|e| e.to_string())?;

    flag_if_anomalous(db, payment.id, wallet.merchant_id, &d).await?;

    // Settle a matching pending payment request. The request may have been
    // created with `asset = 'native'` while Horizon reports `XLM`, so match
    // on the normalised asset rather than the raw string.
    if let Some(memo) = d.memo.as_deref() {
        let pending = payment_requests::find_pending_by_wallet_and_memo(
            db,
            wallet.id,
            memo,
            normalise_asset(&d.asset),
        )
        .await
        .map_err(|e| e.to_string())?;
        if let Some(request) = pending {
            if same_asset(&request.asset, &d.asset) {
                let status = UpdatePaymentStatus {
                    id: request.id,
                    status: "paid".to_string(),
                };
                payment_requests::update_status(db, &status)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_asset_treats_native_and_xlm_as_equal() {
        assert!(same_asset("native", "XLM"));
        assert!(same_asset("XLM", "native"));
        assert!(same_asset("native", "native"));
        assert!(same_asset("xlm", "XLM"));
        assert!(!same_asset("native", "USDC"));
        assert!(!same_asset("XLM", "USDC"));
    }

    #[test]
    fn normalise_asset_collapses_native_to_xlm() {
        assert_eq!(normalise_asset("native"), "XLM");
        assert_eq!(normalise_asset("NATIVE"), "XLM");
        assert_eq!(normalise_asset("XLM"), "XLM");
        assert_eq!(normalise_asset("USDC"), "USDC");
    }
}
