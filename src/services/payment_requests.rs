use chrono::{Duration, Utc};
use rand::RngCore;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::status::PaymentRequestStatus;
use crate::models::PaymentRequest;

const DEFAULT_EXPIRY_SECS: i64 = 15 * 60;
const MIN_EXPIRY_SECS: i64 = 60;
const MAX_EXPIRY_SECS: i64 = 24 * 60 * 60;

/// Retention window after which expired+cancelled rows are hard-deleted.
pub const HARD_DELETE_AFTER_DAYS: i64 = 30;
/// Default edge length (in pixels) of a rendered payment request QR code.
pub const DEFAULT_QR_SIZE: u32 = 256;
/// Smallest QR image we will render, to keep codes scannable.
pub const MIN_QR_SIZE: u32 = 64;
/// Largest QR image we will render, to bound memory/CPU per request.
pub const MAX_QR_SIZE: u32 = 1024;

#[derive(Debug, thiserror::Error)]
pub enum PaymentRequestError {
    #[error("amount_stroops must be positive")]
    InvalidAmount,
    #[error("no wallet found for merchant on network {0}")]
    WalletNotFound(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Random bytes per memo. 14 bytes hex-encode to 28 characters — the most a
/// Stellar `MEMO_TEXT` can carry — giving 112 bits of entropy.
const MEMO_BYTES: usize = 14;

/// Memo collisions are astronomically unlikely at 112 bits, but `memo` is
/// UNIQUE (migration 0004), so a collision is retried with a fresh memo
/// instead of failing the request.
const MEMO_ATTEMPTS: usize = 3;
#[derive(Debug, thiserror::Error)]
pub enum QrError {
    #[error("payment request not found")]
    NotFound,
    #[error("failed to encode QR code: {0}")]
    Encode(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

fn generate_memo() -> String {
    let mut bytes = [0u8; MEMO_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Resolves the wallet a payment request should be routed to for a given
/// merchant and network. Merchants may hold multiple wallets (one per asset /
/// network), so we select the wallet matching the requested network rather than
/// blindly returning the newest wallet by `created_at`.
pub async fn wallet_by_merchant_and_network(
    db: &PgPool,
    merchant_id: Uuid,
    network: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id
           FROM wallets
          WHERE merchant_id = $1 AND network = $2
          ORDER BY created_at DESC
          LIMIT 1",
    )
    .bind(merchant_id)
    .bind(network)
    .fetch_optional(db)
    .await
fn is_memo_collision(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db) if db.is_unique_violation()
        && db.constraint().is_some_and(|c| c.contains("memo")))
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

    sqlx::query_as::<_, PaymentRequest>(
        "INSERT INTO payment_requests (merchant_id, wallet_id, amount_stroops, asset, memo, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, merchant_id, wallet_id, amount_stroops, asset, memo, status, payment_id,
                   expires_at, cancelled_at, created_at, updated_at",
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
    let mut attempt = 1;
    loop {
        let memo = generate_memo();
        let result = sqlx::query_as::<_, PaymentRequest>(
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
        .await;

        match result {
            Err(err) if is_memo_collision(&err) && attempt < MEMO_ATTEMPTS => {
                tracing::warn!(attempt, "payment request memo collision; regenerating");
                attempt += 1;
            }
            other => return other.map_err(PaymentRequestError::from),
        }
    }
}

/// Creates a payment request, routing it to the merchant's wallet that matches
/// the request's asset/network. This lets a merchant hold separate wallets per
/// asset (e.g. XLM and cNGN) and have each request use the correct one.
pub async fn create_payment_request_for_network(
    db: &PgPool,
    merchant_id: Uuid,
    amount_stroops: i64,
    asset: String,
    network: String,
    expires_in_secs: Option<i64>,
) -> Result<PaymentRequest, PaymentRequestError> {
    let wallet_id = wallet_by_merchant_and_network(db, merchant_id, &network)
        .await?
        .ok_or_else(|| PaymentRequestError::WalletNotFound(network.clone()))?;

    create_payment_request(db, merchant_id, wallet_id, amount_stroops, asset, expires_in_secs).await
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
    pub status: PaymentRequestStatus,
    pub payment_id: Option<Uuid>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub cancelled_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub address: String,
    pub network: String,
}

pub async fn payment_requests_by_merchant(
    db: &PgPool,
    merchant_id: Uuid,
    limit: i64,
    include_cancelled: bool,
) -> Result<Vec<PaymentRequestWithWallet>, sqlx::Error> {
    if include_cancelled {
        sqlx::query_as::<_, PaymentRequestWithWallet>(
            "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                    pr.status, pr.payment_id, pr.expires_at, pr.cancelled_at, pr.created_at, pr.updated_at,
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
    } else {
        sqlx::query_as::<_, PaymentRequestWithWallet>(
            "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                    pr.status, pr.payment_id, pr.expires_at, pr.cancelled_at, pr.created_at, pr.updated_at,
                    w.address, w.network
               FROM payment_requests pr
               JOIN wallets w ON w.id = pr.wallet_id
              WHERE pr.merchant_id = $1
                AND pr.cancelled_at IS NULL
              ORDER BY pr.created_at DESC
              LIMIT $2",
        )
        .bind(merchant_id)
        .bind(limit)
        .fetch_all(db)
        .await
    }
    sqlx::query_as::<_, PaymentRequestWithWallet>(
        "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                CASE WHEN pr.status = 'pending' AND pr.expires_at < now() THEN 'expired' ELSE pr.status END AS status,
                pr.payment_id, pr.expires_at, pr.created_at, pr.updated_at,
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
    include_cancelled: bool,
) -> Result<Vec<PaymentRequestWithWallet>, sqlx::Error> {
    match (cursor, include_cancelled) {
        (Some(c), true) => {
            sqlx::query_as::<_, PaymentRequestWithWallet>(
                "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                        pr.status, pr.payment_id, pr.expires_at, pr.cancelled_at, pr.created_at, pr.updated_at,
                        w.address, w.network
                   FROM payment_requests pr
                   JOIN wallets w ON w.id = pr.wallet_id
                  WHERE pr.merchant_id = $1
                    AND w.address IS NOT NULL
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
        (Some(c), false) => {
            sqlx::query_as::<_, PaymentRequestWithWallet>(
                "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                        pr.status, pr.payment_id, pr.expires_at, pr.cancelled_at, pr.created_at, pr.updated_at,
                        CASE WHEN pr.status = 'pending' AND pr.expires_at < now() THEN 'expired' ELSE pr.status END AS status,
                        pr.payment_id, pr.expires_at, pr.created_at, pr.updated_at,
                        w.address, w.network
                   FROM payment_requests pr
                   JOIN wallets w ON w.id = pr.wallet_id
                  WHERE pr.merchant_id = $1
                    AND pr.cancelled_at IS NULL
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
        (None, true) => {
            sqlx::query_as::<_, PaymentRequestWithWallet>(
                "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                        pr.status, pr.payment_id, pr.expires_at, pr.cancelled_at, pr.created_at, pr.updated_at,
                        CASE WHEN pr.status = 'pending' AND pr.expires_at < now() THEN 'expired' ELSE pr.status END AS status,
                        pr.payment_id, pr.expires_at, pr.created_at, pr.updated_at,
                        w.address, w.network
                   FROM payment_requests pr
                   JOIN wallets w ON w.id = pr.wallet_id
                  WHERE pr.merchant_id = $1
                    AND w.address IS NOT NULL
                  ORDER BY pr.created_at DESC, pr.id DESC
                  LIMIT $2",
            )
            .bind(merchant_id)
            .bind(limit + 1)
            .fetch_all(db)
            .await
        }
        (None, false) => {
            sqlx::query_as::<_, PaymentRequestWithWallet>(
                "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                        pr.status, pr.payment_id, pr.expires_at, pr.cancelled_at, pr.created_at, pr.updated_at,
                        w.address, w.network
                   FROM payment_requests pr
                   JOIN wallets w ON w.id = pr.wallet_id
                  WHERE pr.merchant_id = $1
                    AND pr.cancelled_at IS NULL
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

/// Read queries report a `pending` row whose expiry has passed as `expired`,
/// so a request going stale needs no background job to flip it.
pub async fn payment_request_by_id(db: &PgPool, id: Uuid) -> Result<Option<PaymentRequest>, sqlx::Error> {
    sqlx::query_as::<_, PaymentRequest>(
        "SELECT id, merchant_id, wallet_id, amount_stroops, asset, memo, status, payment_id,
                expires_at, cancelled_at, created_at, updated_at
        "SELECT id, merchant_id, wallet_id, amount_stroops, asset, memo,
                CASE WHEN status = 'pending' AND expires_at < now() THEN 'expired' ELSE status END AS status,
                payment_id, expires_at, created_at, updated_at
           FROM payment_requests WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Soft-delete: sets `cancelled_at` for a request owned by `merchant_id`.
/// Returns the updated row, or `None` if it does not exist / is not owned /
/// was already cancelled.
pub async fn cancel_payment_request(
    db: &PgPool,
    id: Uuid,
    merchant_id: Uuid,
) -> Result<Option<PaymentRequest>, sqlx::Error> {
    sqlx::query_as::<_, PaymentRequest>(
        "UPDATE payment_requests
            SET cancelled_at = now(), updated_at = now()
          WHERE id = $1
            AND merchant_id = $2
            AND cancelled_at IS NULL
      RETURNING id, merchant_id, wallet_id, amount_stroops, asset, memo, status, payment_id,
                expires_at, cancelled_at, created_at, updated_at",
    )
    .bind(id)
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}

/// Looks up the pending request a detected deposit's memo correlates to, if any.
/// Cancelled requests are never matched.
pub async fn find_pending_by_wallet_and_memo(
    db: &PgPool,
    wallet_id: Uuid,
    memo: &str,
) -> Result<Option<PaymentRequest>, sqlx::Error> {
    sqlx::query_as::<_, PaymentRequest>(
        "SELECT id, merchant_id, wallet_id, amount_stroops, asset, memo, status, payment_id,
                expires_at, cancelled_at, created_at, updated_at
           FROM payment_requests
          WHERE wallet_id = $1 AND memo = $2 AND status = 'pending'
            AND cancelled_at IS NULL",
          WHERE wallet_id = $1 AND memo = $2 AND status = $3",
    )
    .bind(wallet_id)
    .bind(memo)
    .bind(PaymentRequestStatus::Pending)
    .fetch_optional(db)
    .await
}

pub async fn mark_paid(db: &PgPool, id: Uuid, payment_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE payment_requests SET status = $2, payment_id = $3, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(PaymentRequestStatus::Paid)
    .bind(payment_id)
    .execute(db)
    .await
    .map(|_| ())
}

pub async fn mark_partial(db: &PgPool, id: Uuid, payment_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE payment_requests SET status = $2, payment_id = $3, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(PaymentRequestStatus::Partial)
    .bind(payment_id)
    .execute(db)
    .await
    .map(|_| ())
}

/// Hard-delete expired+cancelled payment requests whose cancellation is older
/// than [`HARD_DELETE_AFTER_DAYS`]. Returns the number of rows removed.
pub async fn hard_delete_expired_cancelled(db: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM payment_requests
          WHERE cancelled_at IS NOT NULL
            AND cancelled_at < now() - interval '30 days'
            AND expires_at < now()",
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_fits_a_stellar_text_memo() {
        let memo = generate_memo();
        assert_eq!(memo.len(), 28, "MEMO_TEXT is limited to 28 bytes");
        assert!(memo.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn memos_do_not_repeat_across_a_large_batch() {
        let memos: std::collections::HashSet<String> = (0..10_000).map(|_| generate_memo()).collect();
        assert_eq!(memos.len(), 10_000);
/// Clamps a caller-supplied `size` query parameter into the supported range,
/// falling back to [`DEFAULT_QR_SIZE`] when absent.
pub fn normalize_qr_size(size: Option<u32>) -> u32 {
    size.unwrap_or(DEFAULT_QR_SIZE).clamp(MIN_QR_SIZE, MAX_QR_SIZE)
}

/// Renders `sep7_uri` as a PNG QR code with an edge length of `size` pixels.
///
/// The QR module grid is scaled up by an integer factor so the resulting image
/// stays crisp, then padded with a quiet zone and centered on a white canvas of
/// exactly `size` x `size` pixels.
pub fn render_qr_png(sep7_uri: &str, size: u32) -> Result<Vec<u8>, QrError> {
    use image::{ImageBuffer, Luma};
    use qrcode::QrCode;

    let size = normalize_qr_size(Some(size));
    let code = QrCode::new(sep7_uri.as_bytes()).map_err(|e| QrError::Encode(e.to_string()))?;
    let modules = code.to_colors();
    let width = code.width() as u32;

    // Quiet zone (4 modules) required by the QR spec for reliable scanning.
    let quiet = 4u32;
    let total_modules = width + quiet * 2;
    let scale = (size / total_modules).max(1);
    let rendered = total_modules * scale;
    let offset = (size.saturating_sub(rendered)) / 2;

    let mut img: ImageBuffer<Luma<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(size, size, Luma([255u8]));

    for y in 0..width {
        for x in 0..width {
            let idx = (y * width + x) as usize;
            if modules[idx] == qrcode::Color::Dark {
                let px0 = offset + (x + quiet) * scale;
                let py0 = offset + (y + quiet) * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = px0 + dx;
                        let py = py0 + dy;
                        if px < size && py < size {
                            img.put_pixel(px, py, Luma([0u8]));
                        }
                    }
                }
            }
        }
    }

    let mut png = Vec::new();
    image::DynamicImage::ImageLuma8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| QrError::Encode(e.to_string()))?;
    Ok(png)
}

/// Builds the SEP-7 URI a wallet should scan to pay this request.
pub fn sep7_uri_for(request: &PaymentRequest, address: &str, network: &str) -> String {
    let amount = request.amount_stroops as f64 / 10_000_000f64;
    format!(
        "web+stellar:pay?destination={}&amount={}&asset_code={}&memo={}&memo_type=text&network={}",
        address, amount, request.asset, request.memo, network
    )
}

/// Renders the QR code image for a payment request, caching the result by
/// `(id, size)` so repeated POS polls don't re-encode the same PNG.
pub async fn payment_request_qr_png(
    db: &PgPool,
    cache: &crate::services::qr_cache::QrCache,
    id: Uuid,
    size: Option<u32>,
) -> Result<Vec<u8>, QrError> {
    let size = normalize_qr_size(size);
    if let Some(png) = cache.get(id, size) {
        return Ok(png);
    }

    let request = payment_request_by_id(db, id)
        .await?
        .ok_or(QrError::NotFound)?;

    let wallet = sqlx::query_as::<_, (String, String)>(
        "SELECT address, network FROM wallets WHERE id = $1",
    )
    .bind(request.wallet_id)
    .fetch_optional(db)
    .await?
    .ok_or(QrError::NotFound)?;

    let uri = sep7_uri_for(&request, &wallet.0, &wallet.1);
    let png = render_qr_png(&uri, size)?;
    cache.insert(id, size, png.clone());
    Ok(png)
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
