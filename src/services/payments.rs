use sqlx::PgPool;
use uuid::Uuid;

use crate::models::status::PaymentStatus;
use crate::models::{NewPayment, Payment, UpdatePaymentStatus};

/// Columns selected for a [`Payment`] row.
const PAYMENT_COLUMNS: &str = "id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status, confirmations, created_at, updated_at, flagged";

#[derive(Debug, thiserror::Error)]
pub enum PaymentError {
    #[error("wallet not found")]
    WalletNotFound,
    #[error("balance row missing for merchant {merchant_id} asset {asset}")]
    BalanceRowMissing { merchant_id: Uuid, asset: String },
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[tracing::instrument(skip_all, err, fields(merchant_id = %payment.merchant_id, tx_hash = %payment.tx_hash))]
pub async fn record_deposit(db: &PgPool, payment: NewPayment) -> Result<Payment, PaymentError> {
    // Dedup on the compound `(tx_hash, wallet_address)` key rather than
    // `tx_hash` alone. A re-org replay (or memo-replay on a clawback) can
    // reuse a transaction hash against a *different* destination address;
    // keying on `tx_hash` only would silently return the original payment
    // row and credit the wrong merchant. The compound key matches the unique
    // index added in migration `0015_unique_payment_wallet_tx_hash.sql`.
    let existing = sqlx::query_as::<_, Payment>(&format!(
        "SELECT {PAYMENT_COLUMNS}
           FROM payments
          WHERE tx_hash = $1 AND wallet_address = $2",
    ))
    .bind(&payment.tx_hash)
    .bind(&payment.wallet_address)
    .fetch_optional(db)
    .await?;

    if let Some(p) = existing {
        return Ok(p);
    }

    sqlx::query_as::<_, Payment>(&format!(
        "INSERT INTO payments (
             merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING {PAYMENT_COLUMNS}",
    ))
    .bind(payment.merchant_id)
    .bind(payment.wallet_id)
    .bind(&payment.wallet_address)
    .bind(&payment.tx_hash)
    .bind(payment.amount_stroops)
    .bind(&payment.asset)
    .bind(&payment.network)
    .bind(PaymentStatus::Detected)
    .fetch_one(db)
    .await
    .map_err(PaymentError::Database)
}

#[tracing::instrument(skip_all, err, fields(payment_id = %id))]
pub async fn set_status(
    db: &PgPool,
    id: Uuid,
    new_status: UpdatePaymentStatus,
) -> Result<Option<Payment>, sqlx::Error> {
    let status = match new_status {
        UpdatePaymentStatus::Verified => PaymentStatus::Verified,
        UpdatePaymentStatus::Confirmed => PaymentStatus::Confirmed,
        UpdatePaymentStatus::Failed => PaymentStatus::Failed,
    };
    sqlx::query_as::<_, Payment>(&format!(
        "UPDATE payments
            SET status = $2, updated_at = now()
          WHERE id = $1
          RETURNING {PAYMENT_COLUMNS}",
    ))
    .bind(id)
    .bind(status)
    .fetch_optional(db)
    .await
}

/// Records a sweep transaction in the payments table with a `sweep` type.
///
/// A sweep moves a merchant's custodial balance to the consolidated
/// settlement wallet (`STELLAR_SYSTEM_WALLET_ADDRESS`). The row is keyed by
/// the on-chain sweep `tx_hash` so retries are idempotent, mirroring
/// [`record_deposit`].
#[allow(clippy::too_many_arguments)]
pub async fn record_sweep(
    db: &PgPool,
    merchant_id: Uuid,
    wallet_id: Uuid,
    wallet_address: &str,
    tx_hash: &str,
    amount_stroops: i64,
    asset: &str,
    network: &str,
) -> Result<Payment, PaymentError> {
    let existing = sqlx::query_as::<_, Payment>(&format!(
        "SELECT {PAYMENT_COLUMNS}
           FROM payments
          WHERE tx_hash = $1",
    ))
    .bind(tx_hash)
    .fetch_optional(db)
    .await?;

    if let Some(p) = existing {
        return Ok(p);
    }

    sqlx::query_as::<_, Payment>(&format!(
        "INSERT INTO payments (
             merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network,
             status, payment_type
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'confirmed', 'sweep')
         RETURNING {PAYMENT_COLUMNS}",
    ))
    .bind(merchant_id)
    .bind(wallet_id)
    .bind(wallet_address)
    .bind(tx_hash)
    .bind(amount_stroops)
    .bind(asset)
    .bind(network)
    .fetch_one(db)
    .await
    .map_err(PaymentError::Database)
}

#[tracing::instrument(skip_all, err, fields(merchant_id = %merchant_id, limit))]
pub async fn payments_by_merchant(
    db: &PgPool,
    merchant_id: Uuid,
    limit: i64,
    before: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<Vec<Payment>, sqlx::Error> {
    if let Some(before_timestamp) = before {
        sqlx::query_as::<_, Payment>(&format!(
            "SELECT {PAYMENT_COLUMNS}
               FROM payments
              WHERE merchant_id = $1 AND created_at < $3
              ORDER BY created_at DESC
              LIMIT $2",
        ))
        .bind(merchant_id)
        .bind(limit)
        .bind(before_timestamp)
        .fetch_all(db)
        .await
    } else {
        sqlx::query_as::<_, Payment>(&format!(
            "SELECT {PAYMENT_COLUMNS}
               FROM payments
              WHERE merchant_id = $1
              ORDER BY created_at DESC
              LIMIT $2",
        ))
        .bind(merchant_id)
        .bind(limit)
        .fetch_all(db)
        .await
    }
}

/// One row of the per-asset, per-day confirmed deposit aggregate produced by
/// [`daily_confirmed_totals`].
///
/// `day` is the UTC calendar day the payment was created on. Rows are sparse:
/// a day with no confirmed payments for an asset is simply absent rather than
/// reported as a zero row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DailyAssetTotal {
    pub asset: String,
    pub day: chrono::NaiveDate,
    pub total_stroops: i64,
    pub count: i64,
}

/// Per-asset summary rolled up from [`DailyAssetTotal`] rows, consumed by the
/// analytics `summary` field.
#[derive(Debug, Clone)]
pub struct AssetDailySummary {
    pub asset: String,
    pub total_stroops: i64,
    pub count: i64,
    pub days: Vec<DailyAssetTotal>,
}

/// Rolls the sparse per-day rows from [`daily_confirmed_totals`] up into one
/// entry per asset, preserving the per-day breakdown. Assets are ordered by
/// descending total amount (ties broken by asset name) so the largest
/// settlement volume appears first; each asset's `days` are ordered by day
/// ascending.
pub fn summarize_daily_totals(rows: Vec<DailyAssetTotal>) -> Vec<AssetDailySummary> {
    use std::collections::BTreeMap;

    let mut by_asset: BTreeMap<String, AssetDailySummary> = BTreeMap::new();
    for row in rows {
        let entry = by_asset
            .entry(row.asset.clone())
            .or_insert_with(|| AssetDailySummary {
                asset: row.asset.clone(),
                total_stroops: 0,
                count: 0,
                days: Vec::new(),
            });
        entry.total_stroops += row.total_stroops;
        entry.count += row.count;
        entry.days.push(row);
    }

    let mut summaries: Vec<AssetDailySummary> = by_asset.into_values().collect();
    for summary in &mut summaries {
        summary.days.sort_by_key(|d| d.day);
    }
    summaries.sort_by(|a, b| {
        b.total_stroops
            .cmp(&a.total_stroops)
            .then_with(|| a.asset.cmp(&b.asset))
    });
    summaries
}

/// All of a merchant's payments created within the optional `[from, to]`
/// window, oldest first — the row source for the CSV export.
pub async fn payments_by_merchant_range(
    db: &PgPool,
    merchant_id: Uuid,
    from: Option<chrono::DateTime<chrono::Utc>>,
    to: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<Vec<Payment>, sqlx::Error> {
    sqlx::query_as::<_, Payment>(&format!(
        "SELECT {PAYMENT_COLUMNS}
           FROM payments
          WHERE merchant_id = $1
            AND ($2::timestamptz IS NULL OR created_at >= $2)
            AND ($3::timestamptz IS NULL OR created_at <= $3)
          ORDER BY created_at ASC",
    ))
    .bind(merchant_id)
    .bind(from)
    .bind(to)
    .fetch_all(db)
    .await
}

/// Keyset-paginated variant of [`payments_by_merchant`]. Orders by
/// `(created_at, id)` DESC so concurrent inserts can't shift rows across
/// pages the way an OFFSET-based scan can. `asset` and `status` are optional
/// filters; they are applied in the same WHERE clause as the keyset predicate
/// so pagination stays correct when filtering.
pub async fn payments_by_merchant_cursor(
    db: &PgPool,
    merchant_id: Uuid,
    limit: i64,
    cursor: Option<crate::pagination::Cursor>,
    asset: Option<&str>,
    status: Option<&str>,
) -> Result<Vec<Payment>, sqlx::Error> {
    sqlx::query_as::<_, Payment>(&format!(
        "SELECT {PAYMENT_COLUMNS}
           FROM payments
          WHERE merchant_id = $1
            AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3::uuid))
            AND ($4::text IS NULL OR asset = $4)
            AND ($5::text IS NULL OR status = $5)
          ORDER BY created_at DESC, id DESC
          LIMIT $6",
    ))
    .bind(merchant_id)
    .bind(cursor.as_ref().map(|c| c.created_at))
    .bind(cursor.as_ref().map(|c| c.id))
    .bind(asset)
    .bind(status)
    .bind(limit + 1)
    .fetch_all(db)
    .await
}

#[tracing::instrument(skip_all, err, fields(payment_id = %id))]
pub async fn payment_by_id(db: &PgPool, id: Uuid) -> Result<Option<Payment>, sqlx::Error> {
    sqlx::query_as::<_, Payment>(&format!(
        "SELECT {PAYMENT_COLUMNS}
           FROM payments
          WHERE id = $1",
    ))
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Merchant notification preferences relevant to deposit alerts.
///
/// Notifications are opt-in: a merchant only receives a deposit email when
/// `deposit_email_enabled` is true. `unsubscribe_token` is a stable, opaque
/// value embedded in the email's unsubscribe link so merchants can opt out
/// without authenticating (GDPR-compliant one-click unsubscribe).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MerchantNotificationPrefs {
    pub merchant_id: Uuid,
    pub email: Option<String>,
    pub deposit_email_enabled: bool,
    pub unsubscribe_token: Uuid,
}

/// Loads the notification preferences for a merchant. Returns `None` when the
/// merchant has no preferences row yet (i.e. has never opted in).
pub async fn merchant_notification_prefs(
    db: &PgPool,
    merchant_id: Uuid,
) -> Result<Option<MerchantNotificationPrefs>, sqlx::Error> {
    sqlx

/* … truncated 3703 chars — edit only what you need near the top … */
