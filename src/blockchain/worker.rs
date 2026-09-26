use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use crate::blockchain::stellar::{BlockchainListener, DetectedDeposit, StellarListener};
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
