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

/// Optional `status` filter for the payment-request list endpoint.
///
/// Valid values are `pending`, `paid`, `partial`, and `expired`. For
/// `pending`, the SQL additionally requires the request to be not yet expired
/// so a POS terminal polling for active requests doesn't see stale rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentRequestStatusFilter {
    Pending,
    Paid,
    Partial,
    Expired,
}

impl PaymentRequestStatusFilter {
    /// Parses a raw `?status=` query value. Returns `Ok(None)` when the value
    /// is absent, and `Err(())` when it is present but not a valid status.
    pub fn parse(raw: Option<&str>) -> Result<Option<Self>, ()> {
        match raw {
            None => Ok(None),
            Some("pending") => Ok(Some(Self::Pending)),
            Some("paid") => Ok(Some(Self::Paid)),
            Some("partial") => Ok(Some(Self::Partial)),
            Some("expired") => Ok(Some(Self::Expired)),
            Some(_) => Err(()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Paid => "paid",
            Self::Partial => "partial",
            Self::Expired => "expired",
        }
    }
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
///
/// When `status` is supplied the filter is applied at the SQL level. For
/// `pending` the query also requires `expires_at > now()` so only active
/// requests are returned. Pagination is unaffected: the keyset predicate and
/// ordering are identical regardless of the filter.
pub async fn payment_requests_by_merchant_cursor(
    db: &PgPool,
    merchant_id: Uuid,
    limit: i64,
    cursor: Option<crate::pagination::Cursor>,
    status: Option<PaymentRequestStatusFilter>,
) -> Result<Vec<PaymentRequestWithWallet>, sqlx::Error> {
    // Build the status predicate once; `$1` is always merchant_id and the
    // cursor/limit placeholders are appended after it.
    let status_clause = match status {
        None => "",
        Some(PaymentRequestStatusFilter::Pending) => {
            " AND pr.status = 'pending' AND pr.expires_at > now()"
        }
        Some(_) => " AND pr.status = $5",
    };

    match cursor {
        Some(c) => {
            let sql = format!(
                "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                        pr.status, pr.payment_id, pr.expires_at, pr.created_at, pr.updated_at,
                        w.address, w.network
                   FROM payment_requests pr
                   JOIN wallets w ON w.id = pr.wallet_id
                  WHERE pr.merchant_id = $1
                    AND (pr.created_at, pr.id) < ($2, $3){status_clause}
                  ORDER BY pr.created_at DESC, pr.id DESC
                  LIMIT $4"
            );
            let mut query = sqlx::query_as::<_, PaymentRequestWithWallet>(&sql)
                .bind(merchant_id)
                .bind(c.created_at)
                .bind(c.id)
                .bind(limit + 1);
            if let Some(s) = status {
                if s != PaymentRequestStatusFilter::Pending {
                    query = query.bind(s.as_str());
                }
            }
            query.fetch_all(db).await
        }
        None => {
            let sql = format!(
                "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                        pr.status, pr.payment_id, pr.expires_at, pr.created_at, pr.updated_at,
                        w.address, w.network
                   FROM payment_requests pr
                   JOIN wallets w ON w.id = pr.wallet_id
                  WHERE pr.merchant_id = $1{status_clause}
                  ORDER BY pr.created_at DESC, pr.id DESC
                  LIMIT $2"
            );
            let mut query = sqlx::query_as::<_, PaymentRequestWithWallet>(&sql)
                .bind(merchant_id)
                .bind(limit + 1);
            if let Some(s) = status {
                if s != PaymentRequestStatusFilter::Pending {
                    query = query.bind(s.as_str());
                }
            }
            query.fetch_all(db).await
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
