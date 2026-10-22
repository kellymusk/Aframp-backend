use sqlx::PgPool;
use uuid::Uuid;

use crate::models::status::PaymentStatus;
use crate::models::{NewPayment, Payment, UpdatePaymentStatus};

/// Columns selected for a [`Payment`] row.
const PAYMENT_COLUMNS: &str = "id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status, confirmations, created_at, updated_at";

#[derive(Debug, thiserror::Error)]
pub enum PaymentError {
    #[error("wallet not found")]
    WalletNotFound,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[tracing::instrument(skip_all, err, fields(merchant_id = %payment.merchant_id, tx_hash = %payment.tx_hash))]
pub async fn record_deposit(db: &PgPool, payment: NewPayment) -> Result<Payment, PaymentError> {
    let existing = sqlx::query_as::<_, Payment>(&format!(
        "SELECT {PAYMENT_COLUMNS}
           FROM payments
          WHERE tx_hash = $1",
    ))
    .bind(&payment.tx_hash)
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
    sqlx::query_as::<_, MerchantNotificationPrefs>(
        "SELECT merchant_id, email, deposit_email_enabled, unsubscribe_token
           FROM merchant_notification_prefs
          WHERE merchant_id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}

/// Opts a merchant in (or out) of deposit email notifications. Creates the
/// preferences row on first call, generating a stable unsubscribe token.
pub async fn set_deposit_email_enabled(
    db: &PgPool,
    merchant_id: Uuid,
    email: &str,
    enabled: bool,
) -> Result<MerchantNotificationPrefs, sqlx::Error> {
    sqlx::query_as::<_, MerchantNotificationPrefs>(
        "INSERT INTO merchant_notification_prefs (
             merchant_id, email, deposit_email_enabled, unsubscribe_token
         )
         VALUES ($1, $2, $3, gen_random_uuid())
         ON CONFLICT (merchant_id) DO UPDATE
            SET email = EXCLUDED.email,
                deposit_email_enabled = EXCLUDED.deposit_email_enabled,
                updated_at = now()
         RETURNING merchant_id, email, deposit_email_enabled, unsubscribe_token",
    )
    .bind(merchant_id)
    .bind(email)
    .bind(enabled)
    .fetch_one(db)
    .await
}

/// Disables deposit email notifications using the opaque unsubscribe token
/// from the email link. Returns `true` when a matching opt-in was disabled.
/// This is the GDPR-compliant one-click unsubscribe path and requires no
/// authentication.
pub async fn unsubscribe_deposit_email(
    db: &PgPool,
    token: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE merchant_notification_prefs
            SET deposit_email_enabled = false, updated_at = now()
          WHERE unsubscribe_token = $1
            AND deposit_email_enabled = true",
    )
    .bind(token)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Adds one confirmation to every deposit still waiting in `verified`. Called
/// once per poll pass, so `confirmations` counts passes since detection.
pub async fn age_verified_deposits(db: &PgPool) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE payments SET confirmations = confirmations + 1, updated_at = now()
          WHERE status = 'verified'",
    )
    .execute(db)
    .await?
    .rows_affected())
}

/// Records the first confirmation of a newly verified deposit.
pub async fn bump_confirmations(db: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE payments SET confirmations = confirmations + 1, updated_at = now()
          WHERE id = $1 AND status = 'verified'",
    )
    .bind(id)
    .execute(db)
    .await
    .map(|_| ())
}

/// Promotes every verified deposit with at least `min_confirmations` to
/// `confirmed`, moving its amount from `pending` to `available` in the same
/// transaction. Returns the promoted payments.
pub async fn promote_confirmed(db: &PgPool, min_confirmations: i32) -> Result<Vec<Payment>, sqlx::Error> {
    let ready = sqlx::query_as::<_, Payment>(&format!(
        "SELECT {PAYMENT_COLUMNS}
           FROM payments
          WHERE status = 'verified' AND confirmations >= $1",
    ))
    .bind(min_confirmations)
    .fetch_all(db)
    .await?;

    let mut promoted = Vec::new();
    for p in ready {
        let mut tx = db.begin().await?;
        // Guarded on status so a concurrent pass can't promote it twice.
        let updated = sqlx::query(
            "UPDATE payments SET status = 'confirmed', updated_at = now()
              WHERE id = $1 AND status = 'verified'",
        )
        .bind(p.id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated == 0 {
            tx.rollback().await?;
            continue;
        }
        sqlx::query(
            "UPDATE balances
                SET available = available + $3, pending = pending - $3, updated_at = now()
              WHERE merchant_id = $1 AND asset = $2",
        )
        .bind(p.merchant_id)
        .bind(&p.asset)
        .bind(p.amount_stroops)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        promoted.push(p);
    }
    Ok(promoted)
}

/// One UTC day's confirmed-payment totals for one asset.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct DailyPaymentAggregate {
    pub date: chrono::NaiveDate,
    pub count: i64,
    pub total_stroops: i64,
    pub asset: String,
}

/// Confirmed payments for `merchant_id` created in `[start, end)`, grouped by
/// UTC calendar day and asset, oldest day first. Days with no payments are
/// absent rather than zero-filled.
pub async fn daily_confirmed_totals(
    db: &PgPool,
    merchant_id: Uuid,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<DailyPaymentAggregate>, sqlx::Error> {
    sqlx::query_as::<_, DailyPaymentAggregate>(
        "SELECT (created_at AT TIME ZONE 'UTC')::date AS date,
                COUNT(*) AS count,
                SUM(amount_stroops)::bigint AS total_stroops,
                asset
           FROM payments
          WHERE merchant_id = $1 AND status = 'confirmed'
            AND created_at >= $2 AND created_at < $3
          GROUP BY 1, asset
          ORDER BY 1, asset",
    )
    .bind(merchant_id)
    .bind(start)
    .bind(end)
    .fetch_all(db)
    .await
}
