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

async fn poll_once(db: &PgPool, listener: &StellarListener) -> Result<(), String> {
    let addresses: Vec<String> = wallets::all_wallet_addresses(db)
        .await
        .map_err(|e| e.to_string())?;
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

/// Moves confirmed merchant funds from the merchant's custodial wallet to the
/// platform settlement wallet (`STELLAR_SYSTEM_WALLET_ADDRESS`).
///
/// The merchant wallet secret is decrypted, used to sign a Stellar transfer to
/// the settlement address, and the resulting sweep is recorded in the payments
/// table with a `sweep` type.
async fn sweep_confirmed_payment(
    db: &PgPool,
    wallet: &crate::models::Wallet,
    payment: &crate::models::Payment,
    asset: &str,
) -> Result<(), String> {
    let settlement_address = std::env::var("STELLAR_SYSTEM_WALLET_ADDRESS")
        .map_err(|_| "STELLAR_SYSTEM_WALLET_ADDRESS is not configured".to_string())?;
    if settlement_address.trim().is_empty() {
        return Err("STELLAR_SYSTEM_WALLET_ADDRESS is empty".into());
    }

    // Decrypt the merchant wallet secret so we can sign the sweep transfer.
    let secret = wallets::decrypt_secret(db, wallet.id)
        .await
        .map_err(|e| e.to_string())?;

    let sweep_tx_hash = crate::blockchain::stellar::submit_transfer(
        &secret,
        &settlement_address,
        payment.amount_stroops,
        asset,
    )
    .await?;

    payments::record_sweep(
        db,
        NewPayment {
            merchant_id: wallet.merchant_id,
            wallet_id: wallet.id,
            wallet_address: wallet.address.clone(),
            tx_hash: sweep_tx_hash,
            amount_stroops: payment.amount_stroops,
            asset: asset.to_string(),
            network: "stellar".into(),
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}
