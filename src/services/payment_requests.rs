use chrono::{DateTime, Duration, Utc};
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

/// Stroops per kobo. cNGN has 2 decimal places on the Stellar network, so the
/// smallest representable unit is 1 kobo = 100,000 stroops (same as withdrawals).
pub const STROOPS_PER_KOBO: i64 = 100_000;

/// Random bytes per memo. 14 bytes hex-encode to 28 characters — the most a
/// Stellar `MEMO_TEXT` can carry — giving 112 bits of entropy.
///
/// Entropy analysis: by the birthday bound a collision becomes likely (~50%)
/// after roughly 2^(112/2) = 2^56 ≈ 7.2e16 memos. Even at one million payment
/// requests per day that is ~200 million years. The previous 8-byte memo
/// (64 bits) hit the same bound after only 2^32 ≈ 4.3e9 requests. 16 bytes
/// (32 hex chars) would overflow `MEMO_TEXT`, so 14 bytes is the ceiling; the
/// `memo` column is unbounded `TEXT`, so no migration is required.
const MEMO_BYTES: usize = 14;

/// Memo collisions are astronomically unlikely at 112 bits, but `memo` is
/// UNIQUE (migration 0004), so a collision is retried with a fresh memo
/// instead of failing the request.
const MEMO_ATTEMPTS: usize = 3;

/// Columns selected for a [`PaymentRequest`] row.
const PR_COLS: &str = "id, merchant_id, wallet_id, amount_stroops, asset, memo, status, payment_id,
                       expires_at, cancelled_at, created_at, updated_at, description";

/// Columns selected for a [`PaymentRequestWithWallet`] row (`pr` joined to `w`).
const PR_WALLET_COLS: &str = "pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                              pr.status, pr.payment_id, pr.expires_at, pr.cancelled_at, pr.created_at,
                              pr.updated_at, pr.description, w.address, w.network";

#[derive(Debug, thiserror::Error)]
pub enum PaymentRequestError {
    #[error("amount_stroops must be positive")]
    InvalidAmount,
    #[error("amount_stroops must be a whole number of the asset's smallest unit (cNGN: {0} stroops per kobo)")]
    InvalidPrecision(i64),
    #[error("expires_in_secs must be between 60 and 86400")]
    InvalidExpiry,
    #[error("no wallet found for merchant on network {0}")]
    WalletNotFound(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Returns the smallest-unit size in stroops for an asset, or `None` when the
/// asset has no sub-stroop precision constraint (e.g. XLM, 7 decimal places).
fn smallest_unit_stroops(asset: &str) -> Option<i64> {
    match asset.to_ascii_uppercase().as_str() {
        "CNGN" => Some(STROOPS_PER_KOBO),
        _ => None,
    }
}

fn generate_memo() -> String {
    let mut bytes = [0u8; MEMO_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn is_memo_collision(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db) if db.is_unique_violation()
        && db.constraint().is_some_and(|c| c.contains("memo")))
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
}

/// A `pending` row whose expiry has passed is reported as `expired` at read
/// time, so a request going stale needs no background job to flip it.
/// Cancelled rows report as `cancelled` regardless of expiry. Lives in the
/// service layer so any caller (a webhook handler, a scheduled job) can use
/// it without importing from `api::payment_requests`.
pub fn effective_status(
    status: PaymentRequestStatus,
    expires_at: DateTime<Utc>,
    cancelled_at: Option<DateTime<Utc>>,
) -> String {
    if cancelled_at.is_some() {
        return "cancelled".to_string();
    }
    if status == PaymentRequestStatus::Pending && expires_at < Utc::now() {
        "expired".to_string()
    } else {
        status.as_str().to_string()
    }
}

#[tracing::instrument(skip_all, err, fields(merchant_id = %merchant_id, wallet_id = %wallet_id, %asset))]
pub async fn create_payment_request(
    db: &PgPool,
    merchant_id: Uuid,
    wallet_id: Uuid,
    amount_stroops: i64,
    asset: String,
    expires_in_secs: Option<i64>,
    description: Option<String>,
) -> Result<PaymentRequest, PaymentRequestError> {
    if amount_stroops <= 0 {
        return Err(PaymentRequestError::InvalidAmount);
    }
    // Reject amounts that are not a whole number of the asset's smallest unit,
    // so the generated SEP-0007 URI carries an amount wallets can represent
    // exactly and the deposit worker can match it against a full-unit deposit.
    if let Some(unit) = smallest_unit_stroops(&asset) {
        if amount_stroops % unit != 0 {
            return Err(PaymentRequestError::InvalidPrecision(unit));
        }
    }
    // Rejected rather than clamped, so a caller never believes a request
    // lives longer (or shorter) than it really does.
    let ttl = expires_in_secs.unwrap_or(DEFAULT_EXPIRY_SECS);
    if !(MIN_EXPIRY_SECS..=MAX_EXPIRY_SECS).contains(&ttl) {
        return Err(PaymentRequestError::InvalidExpiry);
    }
    let expires_at = Utc::now() + Duration::seconds(ttl);

    let mut attempt = 1;
    loop {
        let memo = generate_memo();
        let result = sqlx::query_as::<_, PaymentRequest>(&format!(
            "INSERT INTO payment_requests (merchant_id, wallet_id, amount_stroops, asset, memo, expires_at, description)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING {PR_COLS}"
        ))
        .bind(merchant_id)
        .bind(wallet_id)
        .bind(amount_stroops)
        .bind(&asset)
        .bind(&memo)
        .bind(expires_at)
        .bind(&description)
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
    description: Option<String>,
) -> Result<PaymentRequest, PaymentRequestError> {
    let wallet_id = wallet_by_merchant_and_network(db, merchant_id, &network)
        .await?
        .ok_or_else(|| PaymentRequestError::WalletNotFound(network.clone()))?;

    create_payment_request(
        db,
        merchant_id,
        wallet_id,
        amount_stroops,
        asset,
        expires_in_secs,
        description,
    )
    .await
}

/// A payment request joined with its wallet's address, so listing many doesn't
/// need a wallet lookup per row.
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
    pub description: Option<String>,
    pub address: String,
    pub network: String,
}

/// Lists a merchant's payment requests, newest first. Cancelled (soft-deleted)
/// requests are hidden unless `include_cancelled` is set.
#[tracing::instrument(skip_all, err, fields(merchant_id = %merchant_id, limit))]
pub async fn payment_requests_by_merchant(
    db: &PgPool,
    merchant_id: Uuid,
    limit: i64,
    include_cancelled: bool,
) -> Result<Vec<PaymentRequestWithWallet>, sqlx::Error> {
    sqlx::query_as::<_, PaymentRequestWithWallet>(&format!(
        "SELECT {PR_WALLET_COLS}
           FROM payment_requests pr
           JOIN wallets w ON w.id = pr.wallet_id
          WHERE pr.merchant_id = $1
            AND ($3 OR pr.cancelled_at IS NULL)
         

/* … truncated 6774 chars — edit only what you need near the top … */
