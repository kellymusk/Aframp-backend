use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{NewWithdrawal, Withdrawal};
use crate::payments::{PaymentProvider, PayoutRequest};

/// 1 unit of a Stellar asset = 10,000,000 stroops; 1 Naira = 100 kobo.
/// cNGN is pegged 1:1 to NGN, so 1 kobo = 100,000 stroops.
const STROOPS_PER_KOBO: i64 = 100_000;

#[derive(Debug, thiserror::Error)]
pub enum WithdrawalError {
    #[error("insufficient available balance")]
    InsufficientBalance,
    #[error("withdrawals are only supported for the cNGN asset")]
    UnsupportedAsset,
    #[error("amount_stroops must be a whole number of kobo (a multiple of {STROOPS_PER_KOBO})")]
    InvalidAmountPrecision,
    #[error("daily withdrawal limit exceeded")]
    DailyLimitExceeded,
    #[error("payout provider failed: {0}")]
    PayoutFailed(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

pub async fn create_withdrawal(
    db: &PgPool,
    provider: &dyn PaymentProvider,
    withdrawal: NewWithdrawal,
    daily_limit_stroops: Option<i64>,
) -> Result<Withdrawal, WithdrawalError> {
    if withdrawal.asset != "cNGN" {
        return Err(WithdrawalError::UnsupportedAsset);
    }
    if withdrawal.amount_stroops % STROOPS_PER_KOBO != 0 {
        return Err(WithdrawalError::InvalidAmountPrecision);
    }
    let amount_kobo = withdrawal.amount_stroops / STROOPS_PER_KOBO;

    let mut tx = db.begin().await?;

    if let Some(limit) = daily_limit_stroops {
        let today_completed = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(SUM(amount_stroops), 0)
               FROM withdrawals
              WHERE merchant_id = $1
                AND status = 'completed'
                AND created_at >= date_trunc('day', now() AT TIME ZONE 'UTC')",
        )
        .bind(withdrawal.merchant_id)
        .fetch_one(&mut *tx)
        .await?;

        if today_completed + withdrawal.amount_stroops > limit {
            tx.rollback().await?;
            return Err(WithdrawalError::DailyLimitExceeded);
        }
    }

    let updated = sqlx::query(
        "UPDATE balances
            SET available = available - $2, updated_at = now()
          WHERE merchant_id = $1 AND asset = $3 AND available >= $2",
    )
    .bind(withdrawal.merchant_id)
    .bind(withdrawal.amount_stroops)
    .bind(&withdrawal.asset)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if updated == 0 {
        tx.rollback().await?;
        return Err(WithdrawalError::InsufficientBalance);
    }

    let w = sqlx::query_as::<_, Withdrawal>(
        "INSERT INTO withdrawals (
             merchant_id, amount_stroops, asset, status, bank_code, account_number
         )
         VALUES ($1, $2, $3, 'pending', $4, $5)
         RETURNING id, merchant_id, amount_stroops, asset, status, provider,
                   provider_reference, bank_code, account_number, failure_reason,
                   created_at, updated_at",
    )
    .bind(withdrawal.merchant_id)
    .bind(withdrawal.amount_stroops)
    .bind(&withdrawal.asset)
    .bind(&withdrawal.bank_code)
    .bind(&withdrawal.account_number)
    .fetch_one(&mut *tx)
    .await?;

    // Commit the debit + pending row before ever calling out to Paystack. This
    // guarantees a durable record that the withdrawal was attempted regardless
    // of what happens next — nothing about the external call can make this
    // local state vanish.
    tx.commit().await?;

    let payout = provider
        .create_payout(&PayoutRequest {
            bank_code: withdrawal.bank_code.clone(),
            account_number: withdrawal.account_number.clone(),
            amount: amount_kobo.to_string(),
            reference: w.id.to_string(),
        })
        .await;

    match payout {
        Ok(result) => {
            // If this write fails, the row is left `pending` with no provider
            // info — recoverable later, and safe: it under-states what happened
            // (a real transfer may have gone out) rather than erasing the record
            // that a withdrawal was attempted at all.
            sqlx::query_as::<_, Withdrawal>(
                "UPDATE withdrawals
                    SET provider = $2, provider_reference = $3, status = $4, updated_at = now()
                  WHERE id = $1
                  RETURNING id, merchant_id, amount_stroops, asset, status, provider,
                            provider_reference, bank_code, account_number, failure_reason,
                            created_at, updated_at",
            )
            .bind(w.id)
            .bind(&result.provider)
            .bind(&result.provider_reference)
            .bind(&result.status)
            .fetch_one(db)
            .await
            .map_err(WithdrawalError::Database)
        }
        Err(err) => {
            // Refund + mark failed as one atomic unit, in a fresh transaction —
            // the original debit is already committed, so this is a compensating
            // action, not a rollback. Keeps an audit trail instead of pretending
            // the attempt never happened.
            let mut refund_tx = db.begin().await?;
            sqlx::query(
                "UPDATE balances
                    SET available = available + $2, updated_at = now()
                  WHERE merchant_id = $1 AND asset = $3",
            )
            .bind(withdrawal.merchant_id)
            .bind(withdrawal.amount_stroops)
            .bind(&withdrawal.asset)
            .execute(&mut *refund_tx)
            .await?;

            sqlx::query(
                "UPDATE withdrawals
                    SET status = 'failed', failure_reason = $2, updated_at = now()
                  WHERE id = $1",
            )
            .bind(w.id)
            .bind(&err)
            .execute(&mut *refund_tx)
            .await?;

            refund_tx.commit().await?;
            Err(WithdrawalError::PayoutFailed(err))
        }
    }
}

pub async fn completed_withdrawals_today_stroops(
    db: &PgPool,
    merchant_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(amount_stroops), 0)
           FROM withdrawals
          WHERE merchant_id = $1
            AND status = 'completed'
            AND created_at >= date_trunc('day', now() AT TIME ZONE 'UTC')",
    )
    .bind(merchant_id)
    .fetch_one(db)
    .await
}

pub async fn withdrawals_by_merchant(
    db: &PgPool,
    merchant_id: Uuid,
    limit: i64,
) -> Result<Vec<Withdrawal>, sqlx::Error> {
    sqlx::query_as::<_, Withdrawal>(
        "SELECT id, merchant_id, amount_stroops, asset, status, provider,
                provider_reference, bank_code, account_number, failure_reason,
                created_at, updated_at
           FROM withdrawals
          WHERE merchant_id = $1
          ORDER BY created_at DESC
          LIMIT $2",
    )
    .bind(merchant_id)
    .bind(limit)
    .fetch_all(db)
    .await
}
