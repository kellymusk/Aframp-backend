use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Balance, UpdateBalance};

#[tracing::instrument(skip_all, err, fields(merchant_id = %merchant_id))]
pub async fn get_balances(
    db: &PgPool,
    merchant_id: Uuid,
) -> Result<Vec<Balance>, sqlx::Error> {
    sqlx::query_as::<_, Balance>(
        "SELECT merchant_id, asset, available, pending, updated_at
           FROM balances
          WHERE merchant_id = $1",
    )
    .bind(merchant_id)
    .fetch_all(db)
    .await
}

#[tracing::instrument(skip_all, err, fields(merchant_id = %delta.merchant_id, asset = %delta.asset))]
pub async fn apply_delta(db: &PgPool, delta: &UpdateBalance) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (merchant_id, asset)
         DO UPDATE SET
           available = balances.available + $3,
           pending = balances.pending + $4,
           updated_at = now()",
    )
    .bind(delta.merchant_id)
    .bind(&delta.asset)
    .bind(delta.available_delta)
    .bind(delta.pending_delta)
    .execute(db)
    .await
    .map(|_| ())
}
