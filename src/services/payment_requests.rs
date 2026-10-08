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

/// Random bytes per memo. 14 bytes hex-encode to 28 characters — the most a
/// Stellar `MEMO_TEXT` can carry — giving 112 bits of entropy.
const MEMO_BYTES: usize = 14;

/// Memo collisions are astronomically unlikely at 112 bits, but `memo` is
/// UNIQUE (migration 0004), so a collision is retried with a fresh memo
/// instead of failing the request.
const MEMO_ATTEMPTS: usize = 3;

/// Columns selected for a [`PaymentRequest`] row.
const PR_COLS: &str = "id, merchant_id, wallet_id, amount_stroops, asset, memo, status, payment_id,
                       expires_at, cancelled_at, created_at, updated_at";

/// Columns selected for a [`PaymentRequestWithWallet`] row (`pr` joined to `w`).
const PR_WALLET_COLS: &str = "pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                              pr.status, pr.payment_id, pr.expires_at, pr.cancelled_at, pr.created_at,
                              pr.updated_at, w.address, w.network";

#[derive(Debug, thiserror::Error)]
pub enum PaymentRequestError {
    #[error("amount_stroops must be positive")]
    InvalidAmount,
    #[error("no wallet found for merchant on network {0}")]
    WalletNotFound(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
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
) -> Result<PaymentRequest, PaymentRequestError> {
    if amount_stroops <= 0 {
        return Err(PaymentRequestError::InvalidAmount);
    }
    let ttl = expires_in_secs
        .unwrap_or(DEFAULT_EXPIRY_SECS)
        .clamp(MIN_EXPIRY_SECS, MAX_EXPIRY_SECS);
    let expires_at = Utc::now() + Duration::seconds(ttl);

    let mut attempt = 1;
    loop {
        let memo = generate_memo();
        let result = sqlx::query_as::<_, PaymentRequest>(&format!(
            "INSERT INTO payment_requests (merchant_id, wallet_id, amount_stroops, asset, memo, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING {PR_COLS}"
        ))
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
          ORDER BY pr.created_at DESC
          LIMIT $2"
    ))
    .bind(merchant_id)
    .bind(limit)
    .bind(include_cancelled)
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
    match cursor {
        Some(c) => {
            sqlx::query_as::<_, PaymentRequestWithWallet>(&format!(
                "SELECT {PR_WALLET_COLS}
                   FROM payment_requests pr
                   JOIN wallets w ON w.id = pr.wallet_id
                  WHERE pr.merchant_id = $1
                    AND ($5 OR pr.cancelled_at IS NULL)
                    AND (pr.created_at, pr.id) < ($2, $3)
                  ORDER BY pr.created_at DESC, pr.id DESC
                  LIMIT $4"
            ))
            .bind(merchant_id)
            .bind(c.created_at)
            .bind(c.id)
            .bind(limit + 1)
            .bind(include_cancelled)
            .fetch_all(db)
            .await
        }
        None => {
            sqlx::query_as::<_, PaymentRequestWithWallet>(&format!(
                "SELECT {PR_WALLET_COLS}
                   FROM payment_requests pr
                   JOIN wallets w ON w.id = pr.wallet_id
                  WHERE pr.merchant_id = $1
                    AND ($3 OR pr.cancelled_at IS NULL)
                  ORDER BY pr.created_at DESC, pr.id DESC
                  LIMIT $2"
            ))
            .bind(merchant_id)
            .bind(limit + 1)
            .bind(include_cancelled)
            .fetch_all(db)
            .await
        }
    }
}

#[tracing::instrument(skip_all, err, fields(payment_request_id = %id))]
pub async fn payment_request_by_id(db: &PgPool, id: Uuid) -> Result<Option<PaymentRequest>, sqlx::Error> {
    sqlx::query_as::<_, PaymentRequest>(&format!(
        "SELECT {PR_COLS} FROM payment_requests WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await
}

#[derive(Debug, thiserror::Error)]
pub enum ExpireError {
    #[error("payment request not found")]
    NotFound,
    #[error("only a pending payment request can be expired")]
    NotPending,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Ends a pending request now, merchant-scoped — e.g. the customer abandoned
/// the session and the merchant wants a fresh QR without waiting out the TTL.
/// Expiry stays a read-time state (see [`effective_status`]); this just moves
/// `expires_at` to now, so the row reads `expired` from here on and can no
/// longer be matched by a late deposit.
pub async fn expire(db: &PgPool, id: Uuid, merchant_id: Uuid) -> Result<PaymentRequest, ExpireError> {
    let updated = sqlx::query_as::<_, PaymentRequest>(&format!(
        "UPDATE payment_requests
            SET expires_at = now(), updated_at = now()
          WHERE id = $1 AND merchant_id = $2
            AND status = 'pending' AND expires_at > now() AND cancelled_at IS NULL
      RETURNING {PR_COLS}"
    ))
    .bind(id)
    .bind(merchant_id)
    .fetch_optional(db)
    .await?;
    if let Some(pr) = updated {
        return Ok(pr);
    }
    // Distinguish "not yours / missing" from "not pending" for the caller.
    match payment_request_by_id(db, id).await? {
        Some(pr) if pr.merchant_id == merchant_id => Err(ExpireError::NotPending),
        _ => Err(ExpireError::NotFound),
    }
}

/// Soft-delete: sets `cancelled_at` for a request owned by `merchant_id`.
/// Returns the updated row, or `None` if it does not exist / is not owned /
/// was already cancelled.
pub async fn cancel_payment_request(
    db: &PgPool,
    id: Uuid,
    merchant_id: Uuid,
) -> Result<Option<PaymentRequest>, sqlx::Error> {
    sqlx::query_as::<_, PaymentRequest>(&format!(
        "UPDATE payment_requests
            SET cancelled_at = now(), updated_at = now()
          WHERE id = $1
            AND merchant_id = $2
            AND cancelled_at IS NULL
      RETURNING {PR_COLS}"
    ))
    .bind(id)
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}

/// Looks up the pending request a detected deposit's memo correlates to, if any.
/// Expired and cancelled requests are never matched.
#[tracing::instrument(skip_all, err, fields(wallet_id = %wallet_id))]
pub async fn find_pending_by_wallet_and_memo(
    db: &PgPool,
    wallet_id: Uuid,
    memo: &str,
) -> Result<Option<PaymentRequest>, sqlx::Error> {
    sqlx::query_as::<_, PaymentRequest>(&format!(
        "SELECT {PR_COLS}
           FROM payment_requests
          WHERE wallet_id = $1 AND memo = $2 AND status = $3
            AND expires_at > now()
            AND cancelled_at IS NULL"
    ))
    .bind(wallet_id)
    .bind(memo)
    .bind(PaymentRequestStatus::Pending)
    .fetch_optional(db)
    .await
}

#[tracing::instrument(skip_all, err, fields(payment_request_id = %id, %payment_id))]
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

#[tracing::instrument(skip_all, err, fields(payment_request_id = %id, %payment_id))]
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
            AND cancelled_at < now() - make_interval(days => $1::int)
            AND expires_at < now()",
    )
    .bind(HARD_DELETE_AFTER_DAYS as i32)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

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
    }
}
