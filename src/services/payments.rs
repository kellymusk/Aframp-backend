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
