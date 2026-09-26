use chrono::{Duration, Utc};
use rand::RngCore;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::PaymentRequest;

const DEFAULT_EXPIRY_SECS: i64 = 15 * 60;
const MIN_EXPIRY_SECS: i64 = 60;
const MAX_EXPIRY_SECS: i64 = 24 * 60 * 60;

#[derive(Debug, thiserror::Error)]
pub enum PaymentRequestError {
    #[error("amount_stroops must be positive")]
    InvalidAmount,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

fn generate_memo() -> String {
    let mut bytes = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub async fn create_payment_request(
    db: &PgPool,
    merchant_id: Uuid,
    wallet_id: Uuid,
    amount_stroops: i64,
    asset: String,
    expires_in_secs: Option<i64>,
) -> Result<PaymentRequest, PaymentRequestError> {
    if amount_stroops <= 0 {
        return Err(PaymentRequestError::InvalidAmount);
    }
    let ttl = expires_in_secs
        .unwrap_or(DEFAULT_EXPIRY_SECS)
        .clamp(MIN_EXPIRY_SECS, MAX_EXPIRY_SECS);
    let expires_at = Utc::now() + Duration::seconds(ttl);
    let memo = generate_memo();

    sqlx::query_as::<_, PaymentRequest>(
        "INSERT INTO payment_requests (merchant_id, wallet_id, amount_stroops, asset, memo, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, merchant_id, wallet_id, amount_stroops, asset, memo, status, payment_id,
                   expires_at, created_at, updated_at",
    )
    .bind(merchant_id)
    .bind(wallet_id)
    .bind(amount_stroops)
    .bind(&asset)
    .bind(&memo)
    .bind(expires_at)
    .fetch_one(db)
    .await
    .map_err(PaymentRequestError::from)
}

/// A payment request joined with its wallet's address, so listing many doesn't
/// fan out into one wallet lookup per row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PaymentRequestWithWallet {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub wallet_id: Uuid,
    pub amount_stroops: i64,
    pub asset: String,
    pub memo: String,
    pub status: String,
    pub payment_id: Option<Uuid>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub address: String,
    pub network: String,
}

pub async fn payment_requests_by_merchant(
    db: &PgPool,
    merchant_id: Uuid,
    limit: i64,
) -> Result<Vec<PaymentRequestWithWallet>, sqlx::Error> {
    sqlx::query_as::<_, PaymentRequestWithWallet>(
        "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                pr.status, pr.payment_id, pr.expires_at, pr.created_at, pr.updated_at,
                w.address, w.network
           FROM payment_requests pr
           JOIN wallets w ON w.id = pr.wallet_id
          WHERE pr.merchant_id = $1
          ORDER BY pr.created_at DESC
          LIMIT $2",
    )
    .bind(merchant_id)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Keyset-paginated variant of [`payment_requests_by_merchant`]. Orders by
/// `(created_at, id)` DESC so concurrent inserts can't shift rows across
/// pages the way an OFFSET-based scan can.
pub async fn payment_requests_by_merchant_cursor(
    db: &PgPool,
    merchant_id: Uuid,
    limit: i64,
    cursor: Option<crate::pagination::Cursor>,
) -> Result<Vec<PaymentRequestWithWallet>, sqlx::Error> {
    match cursor {
        Some(c) => {
            sqlx::query_as::<_, PaymentRequestWithWallet>(
                "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                        pr.status, pr.payment_id, pr.expires_at, pr.created_at, pr.updated_at,
                        w.address, w.network
                   FROM payment_requests pr
                   JOIN wallets w ON w.id = pr.wallet_id
                  WHERE pr.merchant_id = $1
                    AND (pr.created_at, pr.id) < ($2, $3)
                  ORDER BY pr.created_at DESC, pr.id DESC
                  LIMIT $4",
            )
            .bind(merchant_id)
            .bind(c.created_at)
            .bind(c.id)
            .bind(limit + 1)
            .fetch_all(db)
            .await
        }
        None => {
            sqlx::query_as::<_, PaymentRequestWithWallet>(
                "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                        pr.status, pr.payment_id, pr.expires_at, pr.created_at, pr.updated_at,
                        w.address, w.network
                   FROM payment_requests pr
                   JOIN wallets w ON w.id = pr.wallet_id
                  WHERE pr.merchant_id = $1
                  ORDER BY pr.created_at DESC, pr.id DESC
                  LIMIT $2",
            )
            .bind(merchant_id)
            .bind(limit + 1)
            .fetch_all(db)
            .await
        }
    }
}

pub async fn payment_request_by_id(db: &PgPool, id: Uuid) -> Result<Option<PaymentRequest>, sqlx::Error> {
    sqlx::query_as::<_, PaymentRequest>(
        "SELECT id, merchant_id, wallet_id, amount_stroops, asset, memo, status, payment_id,
                expires_at, created_at, updated_at
           FROM payment_requests WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Looks up the pending request a detected deposit's memo correlates to, if any.
pub async fn find_pending_by_wallet_and_memo(
    db: &PgPool,
    wallet_id: Uuid,
    memo: &str,
) -> Result<Option<PaymentRequest>, sqlx::Error> {
    sqlx::query_as::<_, PaymentRequest>(
        "SELECT id, merchant_id, wallet_id, amount_stroops, asset, memo, status, payment_id,
                expires_at, created_at, updated_at
           FROM payment_requests
          WHERE wallet_id = $1 AND memo = $2 AND status = 'pending'",
    )
    .bind(wallet_id)
    .bind(memo)
    .fetch_optional(db)
    .await
}

pub async fn mark_paid(db: &PgPool, id: Uuid, payment_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE payment_requests SET status = 'paid', payment_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(payment_id)
    .execute(db)
    .await
    .map(|_| ())
}

pub async fn mark_partial(db: &PgPool, id: Uuid, payment_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE payment_requests SET status = 'partial', payment_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(payment_id)
    .execute(db)
    .await
    .map(|_| ())
}

#[cfg(test)]
mod onboarding_e2e_tests {
    use super::*;
    use crate::otp::mock::MockOtpProvider;
    use crate::otp::OtpProvider;
    use sqlx::PgPool;
    use uuid::Uuid;

    /// Minimal stand-in for the production blockchain listener. The e2e test
    /// injects a single fake deposit through this trait instead of talking to
    /// a real chain, so the handoff from "deposit detected" to "request paid"
    /// and "balance credited" is exercised deterministically.
    #[async_trait::async_trait]
    trait BlockchainListener {
        async fn inject_deposit(
            &self,
            db: &PgPool,
            wallet_id: Uuid,
            memo: &str,
            amount_stroops: i64,
        ) -> Result<Uuid, sqlx::Error>;
    }

    struct MockBlockchainListener;

    #[async_trait::async_trait]
    impl BlockchainListener for MockBlockchainListener {
        async fn inject_deposit(
            &self,
            db: &PgPool,
            wallet_id: Uuid,
            memo: &str,
            amount_stroops: i64,
        ) -> Result<Uuid, sqlx::Error> {
            // Record the deposit, then correlate it to the pending request via
            // its memo and mark that request paid — mirroring what the real
            // listener does when it observes an on-chain transfer.
            let payment_id: Uuid = sqlx::query_scalar(
                "INSERT INTO payments (wallet_id, amount_stroops, asset, memo, status)
                 VALUES ($1, $2, 'XLM', $3, 'confirmed')
                 RETURNING id",
            )
            .bind(wallet_id)
            .bind(amount_stroops)
            .bind(memo)
            .fetch_one(db)
            .await?;

            if let Some(pr) = find_pending_by_wallet_and_memo(db, wallet_id, memo).await? {
                mark_paid(db, pr.id, payment_id).await?;
            }

            sqlx::query(
                "UPDATE wallets SET balance_stroops = balance_stroops + $2 WHERE id = $1",
            )
            .bind(wallet_id)
            .bind(amount_stroops)
            .execute(db)
            .await?;

            Ok(payment_id)
        }
    }

    /// End-to-end merchant onboarding: signup -> OTP verify -> create wallet ->
    /// create payment request -> detect deposit -> check balance.
    #[sqlx::test]
    async fn full_merchant_onboarding_flow(db: PgPool) {
        let otp = MockOtpProvider::new();
        let listener = MockBlockchainListener;

        // 1. Signup.
        let email = "merchant@example.com";
        let merchant_id: Uuid = sqlx::query_scalar(
            "INSERT INTO merchants (email, name, status)
             VALUES ($1, 'Test Merchant', 'pending')
             RETURNING id",
        )
        .bind(email)
        .fetch_one(&db)
        .await
        .expect("merchant signup should succeed");

        // 2. OTP verify (MockOtpProvider skips real SMS).
        let code = otp
            .send_code(email)
            .await
            .expect("mock OTP send should succeed");
        let verified = otp
            .verify_code(email, &code)
            .await
            .expect("mock OTP verify should succeed");
        assert!(verified, "OTP verification should succeed");

        sqlx::query("UPDATE merchants SET status = 'active' WHERE id = $1")
            .bind(merchant_id)
            .execute(&db)
            .await
            .expect("merchant activation should succeed");

        // 3. Create wallet.
        let wallet_id: Uuid = sqlx::query_scalar(
            "INSERT INTO wallets (merchant_id, address, network, balance_stroops)
             VALUES ($1, 'GTESTWALLETADDRESS', 'testnet', 0)
             RETURNING id",
        )
        .bind(merchant_id)
        .fetch_one(&db)
        .await
        .expect("wallet creation should succeed");

        // 4. Create payment request.
        let amount_stroops: i64 = 10_000_000;
        let pr = create_payment_request(
            &db,
            merchant_id,
            wallet_id,
            amount_stroops,
            "XLM".to_string(),
            None,
        )
        .await
        .expect("payment request creation should succeed");
        assert_eq!(pr.status, "pending");

        // 5. Detect deposit via the mock listener.
        listener
            .inject_deposit(&db, wallet_id, &pr.memo, amount_stroops)
            .await
            .expect("deposit injection should succeed");

        // 6. Check balance equals the deposit amount.
        let balance: i64 = sqlx::query_scalar(
            "SELECT balance_stroops FROM wallets WHERE id = $1",
        )
        .bind(wallet_id)
        .fetch_one(&db)
        .await
        .expect("balance lookup should succeed");
        assert_eq!(balance, amount_stroops, "balance should equal the deposit");

        // And the payment request should now be marked paid.
        let updated = payment_request_by_id(&db, pr.id)
            .await
            .expect("payment request lookup should succeed")
            .expect("payment request should exist");
        assert_eq!(updated.status, "paid");
        assert!(updated.payment_id.is_some());
    }
}
