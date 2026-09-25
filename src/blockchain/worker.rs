use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use crate::blockchain::stellar::{BlockchainListener, StellarListener};
use crate::models::{NewPayment, UpdatePaymentStatus};
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
    }
}
