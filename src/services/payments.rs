use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{NewPayment, Payment, UpdatePaymentStatus};

#[derive(Debug, thiserror::Error)]
pub enum PaymentError {
    #[error("wallet not found")]
    WalletNotFound,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

pub async fn record_deposit(db: &PgPool, payment: NewPayment) -> Result<Payment, PaymentError> {
    let existing = sqlx::query_as::<_, Payment>(
        "SELECT id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset,
                network, status, confirmations, created_at, updated_at
           FROM payments
          WHERE tx_hash = $1",
    )
    .bind(&payment.tx_hash)
    .fetch_optional(db)
    .await?;

    if let Some(p) = existing {
        return Ok(p);
    }

    sqlx::query_as::<_, Payment>(
        "INSERT INTO payments (
             merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'detected')
         RETURNING id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset,
                   network, status, confirmations, created_at, updated_at",
    )
    .bind(payment.merchant_id)
    .bind(payment.wallet_id)
    .bind(&payment.wallet_address)
    .bind(&payment.tx_hash)
    .bind(payment.amount_stroops)
    .bind(&payment.asset)
    .bind(&payment.network)
    .fetch_one(db)
    .await
    .map_err(PaymentError::Database)
}

pub async fn set_status(
    db: &PgPool,
    id: Uuid,
    new_status: UpdatePaymentStatus,
) -> Result<Option<Payment>, sqlx::Error> {
    let status = match new_status {
        UpdatePaymentStatus::Verified => "verified",
        UpdatePaymentStatus::Confirmed => "confirmed",
        UpdatePaymentStatus::Failed => "failed",
    };
    sqlx::query_as::<_, Payment>(
        "UPDATE payments
            SET status = $2, updated_at = now()
          WHERE id = $1
          RETURNING id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset,
                    network, status, confirmations, created_at, updated_at",
    )
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
    let existing = sqlx::query_as::<_, Payment>(
        "SELECT id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset,
                network, status, confirmations, created_at, updated_at
           FROM payments
          WHERE tx_hash = $1",
    )
    .bind(tx_hash)
    .fetch_optional(db)
    .await?;

    if let Some(p) = existing {
        return Ok(p);
    }

    sqlx::query_as::<_, Payment>(
        "INSERT INTO payments (
             merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network,
             status, payment_type
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'confirmed', 'sweep')
         RETURNING id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset,
                   network, status, confirmations, created_at, updated_at",
    )
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

pub async fn payments_by_merchant(
    db: &PgPool,
    merchant_id: Uuid,
    limit: i64,
) -> Result<Vec<Payment>, sqlx::Error> {
    sqlx::query_as::<_, Payment>(
        "SELECT id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset,
                network, status, confirmations, created_at, updated_at
           FROM payments
          WHERE merchant_id = $1
          ORDER BY created_at DESC
          LIMIT $2",
    )
    .bind(merchant_id)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Keyset-paginated variant of [`payments_by_merchant`]. Orders by
/// `(created_at, id)` DESC so concurrent inserts can't shift rows across
/// pages the way an OFFSET-based scan can.
pub async fn payments_by_merchant_cursor(
    db: &PgPool,
    merchant_id: Uuid,
    limit: i64,
    cursor: Option<crate::pagination::Cursor>,
) -> Result<Vec<Payment>, sqlx::Error> {
    match cursor {
        Some(c) => {
            sqlx::query_as::<_, Payment>(
                "SELECT id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset,
                        network, status, confirmations, created_at, updated_at
                   FROM payments
                  WHERE merchant_id = $1
                    AND (created_at, id) < ($2, $3)
                  ORDER BY created_at DESC, id DESC
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
            sqlx::query_as::<_, Payment>(
                "SELECT id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset,
                        network, status, confirmations, created_at, updated_at
                   FROM payments
                  WHERE merchant_id = $1
                  ORDER BY created_at DESC, id DESC
                  LIMIT $2",
            )
            .bind(merchant_id)
            .bind(limit + 1)
            .fetch_all(db)
            .await
        }
    }
}

pub async fn payment_by_id(db: &PgPool, id: Uuid) -> Result<Option<Payment>, sqlx::Error> {
    sqlx::query_as::<_, Payment>(
        "SELECT id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset,
                network, status, confirmations, created_at, updated_at
           FROM payments
          WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Records that a verified deposit was seen again on a later poll pass.
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
/// transaction. Returns how many were promoted.
pub async fn promote_confirmed(db: &PgPool, min_confirmations: i32) -> Result<u64, sqlx::Error> {
    let ready = sqlx::query_as::<_, Payment>(
        "SELECT id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset,
                network, status, confirmations, created_at, updated_at
           FROM payments
          WHERE status = 'verified' AND confirmations >= $1",
    )
    .bind(min_confirmations)
    .fetch_all(db)
    .await?;

    let mut promoted = 0;
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
        promoted += 1;
    }
    Ok(promoted)
}
