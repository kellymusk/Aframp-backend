use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Balance, UpdateBalance};

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

/// Credits a confirmed deposit directly to the merchant's available balance in a
/// single UPSERT, avoiding the two `apply_delta` round-trips (pending credit then
/// pending -> available move) that were previously issued per deposit.
///
/// TODO(#1063): once the confirmation depth feature lands, deposits will be
/// credited to `pending` on first observation and only moved to `available`
/// after the required confirmations. At that point this helper should be split
/// back into two `apply_delta` calls (pending_delta = amount, then
/// pending_delta = -amount / available_delta = amount) so the intermediate
/// pending state is observable.
pub async fn credit_confirmed_deposit(
    db: &PgPool,
    merchant_id: Uuid,
    asset: &str,
    amount: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1, $2, $3, 0)
         ON CONFLICT (merchant_id, asset)
         DO UPDATE SET
           available = balances.available + $3,
           updated_at = now()",
    )
    .bind(merchant_id)
    .bind(asset)
    .bind(amount)
    .execute(db)
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn credit_confirmed_deposit_sets_available_balance(db: PgPool) {
        let merchant_id = Uuid::new_v4();

        credit_confirmed_deposit(&db, merchant_id, "USDC", 1_000)
            .await
            .expect("first deposit credit should succeed");

        let balances = get_balances(&db, merchant_id)
            .await
            .expect("balances should be readable");
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].asset, "USDC");
        assert_eq!(balances[0].available, 1_000);
        assert_eq!(balances[0].pending, 0);

        // A second deposit should accumulate onto the existing row.
        credit_confirmed_deposit(&db, merchant_id, "USDC", 500)
            .await
            .expect("second deposit credit should succeed");

        let balances = get_balances(&db, merchant_id)
            .await
            .expect("balances should be readable");
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].available, 1_500);
        assert_eq!(balances[0].pending, 0);
    }
}
