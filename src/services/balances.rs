use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Balance, UpdateBalance};

pub async fn get_balances(
    db: &PgPool,
    merchant_id: Uuid,
) -> Result<Vec<Balance>, sqlx::Error> {
    sqlx::query_as!(
        Balance,
        "SELECT merchant_id, asset, available, pending, updated_at
           FROM balances
          WHERE merchant_id = $1",
        merchant_id
    )
    .fetch_all(db)
    .await
}

/// Applies a balance delta without any idempotency guard.
///
/// Prefer [`apply_delta_idempotent`] when the delta is derived from a payment
/// so that a partial write or a re-entrant poll cycle cannot double-credit.
pub async fn apply_delta(db: &PgPool, delta: &UpdateBalance) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (merchant_id, asset)
         DO UPDATE SET
           available = balances.available + $3,
           pending = balances.pending + $4,
           updated_at = now()",
        delta.merchant_id,
        delta.asset,
        delta.available_delta,
        delta.pending_delta
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// Applies a balance delta at most once per `(payment_id, kind)`.
///
/// Defense-in-depth for issue #1195: even if the worker logic regresses and
/// calls this twice for the same payment (e.g. a payment stuck in `detected`
/// after a crash between `record_deposit` and `set_status(Verified)`), the
/// `balance_credits` unique constraint makes the second call a no-op.
///
/// `kind` distinguishes the individual balance mutations for a payment, e.g.
/// `"pending_credit"` for the initial detected credit and `"promote"` for the
/// pending -> available promotion once the deposit is verified.
///
/// The credit marker and the balance mutation are performed in a single
/// transaction so the marker is only committed if the balance update succeeds.
pub async fn apply_delta_idempotent(
    db: &PgPool,
    delta: &UpdateBalance,
    payment_id: Uuid,
    kind: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;

    // Insert the idempotency marker first. If this payment/kind has already
    // been credited, the unique constraint makes this a no-op and we skip the
    // balance mutation entirely.
    let inserted = sqlx::query(
        "INSERT INTO balance_credits (payment_id, kind)
         VALUES ($1, $2)
         ON CONFLICT (payment_id, kind) DO NOTHING",
    )
    .bind(payment_id)
    .bind(kind)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if inserted == 0 {
        // Already credited for this payment/kind; nothing to do.
        tx.rollback().await?;
        return Ok(());
    }

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
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
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
