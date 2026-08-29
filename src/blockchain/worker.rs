use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use sqlx::PgPool;

use crate::blockchain::stellar::{BlockchainListener, DepositPoll, StellarListener};
use crate::models::{NewPayment, UpdateBalance, UpdatePaymentStatus};
use crate::services::{balances, payment_requests, payments, wallets};
use crate::AppState;

/// Initial backoff applied to a wallet that returned 404 (unfunded), doubled on
/// each subsequent 404 up to [`BACKOFF_MAX_SECS`].
const BACKOFF_BASE_SECS: u64 = 30;
const BACKOFF_MAX_SECS: u64 = 1800; // 30 minutes

pub async fn run(state: Arc<AppState>, horizon_url: String, poll_interval_secs: u64) {
    let listener = StellarListener { horizon_url };
    // address -> (consecutive 404s seen, earliest instant we may poll again).
    let mut backoff: HashMap<String, (u32, Instant)> = HashMap::new();

    loop {
        if let Err(err) = poll_once(&state.db, &listener, &mut backoff).await {
            tracing::warn!(error = %err, "deposit poll failed");
        }
        tokio::time::sleep(StdDuration::from_secs(poll_interval_secs)).await;
    }
}

async fn poll_once(
    db: &PgPool,
    listener: &StellarListener,
    backoff: &mut HashMap<String, (u32, Instant)>,
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

    // Skip wallets currently in backoff (unfunded / 404) until their window opens.
    let now = Instant::now();
    let due: Vec<String> = addresses
        .into_iter()
        .filter(|a| backoff.get(a).map(|(_, next)| now >= *next).unwrap_or(true))
        .collect();
    if due.is_empty() {
        return Ok(());
    }

    let DepositPoll { deposits, unfunded } = listener.fetch_deposits(&due).await?;
    for deposit in deposits {
        if let Err(err) = process_deposit(db, deposit).await {
            tracing::warn!(error = %err, "failed to process deposit");
        }
    }

    // A wallet that did not 404 this cycle is reachable/funded: drop any backoff
    // it had so it returns to the normal poll cadence.
    let unfunded_set: HashSet<&String> = unfunded.iter().collect();
    for address in &due {
        if !unfunded_set.contains(address) {
            backoff.remove(address);
        }
    }

    // Exponential backoff for wallets that returned 404 this cycle.
    for address in unfunded {
        let entry = backoff.entry(address).or_insert((0, now));
        entry.0 += 1;
        entry.1 = now + backoff_delay(entry.0);
    }
    Ok(())
}

/// Maps consecutive 404s to a backoff interval: base 30s, doubling each time,
/// capped at 30 minutes.
fn backoff_delay(attempt: u32) -> StdDuration {
    let factor = 2u64.saturating_pow(attempt.saturating_sub(1));
    let secs = BACKOFF_BASE_SECS.saturating_mul(factor);
    StdDuration::from_secs(secs.min(BACKOFF_MAX_SECS))
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
            asset: d.asset,
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
            if pr.amount_stroops != payment.amount_stroops {
                tracing::warn!(
                    expected = pr.amount_stroops,
                    actual = payment.amount_stroops,
                    request_id = %pr.id,
                    "payment request amount mismatch — marking paid anyway"
                );
            }
            payment_requests::mark_paid(db, pr.id, payment.id)
                .await
                .map_err(|e| e.to_string())?;
        }
    }

    // TODO: dispatch payment.confirmed webhook.
    Ok(())
}
