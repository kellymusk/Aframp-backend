use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{NewWithdrawal, Withdrawal};
use crate::payments::{PaymentProvider, PayoutRequest, PayoutVerification};

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
    #[error("payout provider failed: {0}")]
    PayoutFailed(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconciliationReport {
    pub total_scanned: usize,
    pub completed: usize,
    pub processing: usize,
    pub pending: usize,
    pub failed_and_refunded: usize,
    pub errors: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconciledStatus {
    Completed,
    Processing,
    Pending,
    FailedAndRefunded,
}

pub async fn create_withdrawal(
    db: &PgPool,
    provider: &dyn PaymentProvider,
    withdrawal: NewWithdrawal,
) -> Result<Withdrawal, WithdrawalError> {
    if withdrawal.asset != "cNGN" {
        return Err(WithdrawalError::UnsupportedAsset);
    }
    if withdrawal.amount_stroops % STROOPS_PER_KOBO != 0 {
        return Err(WithdrawalError::InvalidAmountPrecision);
    }
    let amount_kobo = withdrawal.amount_stroops / STROOPS_PER_KOBO;

    let mut tx = db.begin().await?;

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

pub async fn find_pending_withdrawals_older_than(
    db: &PgPool,
    older_than: chrono::Duration,
) -> Result<Vec<Withdrawal>, sqlx::Error> {
    let cutoff = chrono::Utc::now() - older_than;
    sqlx::query_as::<_, Withdrawal>(
        "SELECT id, merchant_id, amount_stroops, asset, status, provider,
                provider_reference, bank_code, account_number, failure_reason,
                created_at, updated_at
           FROM withdrawals
          WHERE status = 'pending'
            AND created_at <= $1
          ORDER BY created_at ASC",
    )
    .bind(cutoff)
    .fetch_all(db)
    .await
}

pub async fn reconcile_single_withdrawal(
    db: &PgPool,
    provider: &dyn PaymentProvider,
    w: &Withdrawal,
) -> Result<ReconciledStatus, WithdrawalError> {
    let verification = provider
        .verify_payout(&w.id.to_string())
        .await
        .map_err(WithdrawalError::PayoutFailed)?;

    match verification {
        PayoutVerification::Completed {
            provider,
            provider_reference,
        } => {
            sqlx::query(
                "UPDATE withdrawals
                    SET provider = $2, provider_reference = $3, status = 'completed', updated_at = now()
                  WHERE id = $1 AND status = 'pending'",
            )
            .bind(w.id)
            .bind(&provider)
            .bind(&provider_reference)
            .execute(db)
            .await?;

            tracing::info!(
                withdrawal_id = %w.id,
                merchant_id = %w.merchant_id,
                provider = %provider,
                provider_reference = %provider_reference,
                "reconciled pending withdrawal as completed"
            );
            Ok(ReconciledStatus::Completed)
        }
        PayoutVerification::Processing {
            provider,
            provider_reference,
        } => {
            sqlx::query(
                "UPDATE withdrawals
                    SET provider = $2, provider_reference = $3, status = 'processing', updated_at = now()
                  WHERE id = $1 AND status = 'pending'",
            )
            .bind(w.id)
            .bind(&provider)
            .bind(&provider_reference)
            .execute(db)
            .await?;

            tracing::info!(
                withdrawal_id = %w.id,
                merchant_id = %w.merchant_id,
                "reconciled pending withdrawal as processing"
            );
            Ok(ReconciledStatus::Processing)
        }
        PayoutVerification::Pending {
            provider,
            provider_reference,
        } => {
            sqlx::query(
                "UPDATE withdrawals
                    SET provider = $2, provider_reference = $3, updated_at = now()
                  WHERE id = $1 AND status = 'pending'",
            )
            .bind(w.id)
            .bind(&provider)
            .bind(&provider_reference)
            .execute(db)
            .await?;

            tracing::info!(
                withdrawal_id = %w.id,
                merchant_id = %w.merchant_id,
                "pending withdrawal remains pending on provider"
            );
            Ok(ReconciledStatus::Pending)
        }
        PayoutVerification::Failed {
            provider,
            provider_reference,
            reason,
        } => {
            let mut tx = db.begin().await?;

            let updated = sqlx::query(
                "UPDATE withdrawals
                    SET provider = $2, provider_reference = $3, status = 'failed', failure_reason = $4, updated_at = now()
                  WHERE id = $1 AND status = 'pending'",
            )
            .bind(w.id)
            .bind(&provider)
            .bind(&provider_reference)
            .bind(&reason)
            .execute(&mut *tx)
            .await?
            .rows_affected();

            if updated > 0 {
                sqlx::query(
                    "UPDATE balances
                        SET available = available + $2, updated_at = now()
                      WHERE merchant_id = $1 AND asset = $3",
                )
                .bind(w.merchant_id)
                .bind(w.amount_stroops)
                .bind(&w.asset)
                .execute(&mut *tx)
                .await?;

                tx.commit().await?;
                tracing::info!(
                    withdrawal_id = %w.id,
                    merchant_id = %w.merchant_id,
                    reason = %reason,
                    "reconciled pending withdrawal as failed and refunded balance"
                );
            } else {
                tx.rollback().await?;
            }

            Ok(ReconciledStatus::FailedAndRefunded)
        }
        PayoutVerification::NotFound => {
            let reason = "withdrawal not found on payment provider during reconciliation".to_string();
            let mut tx = db.begin().await?;

            let updated = sqlx::query(
                "UPDATE withdrawals
                    SET status = 'failed', failure_reason = $2, updated_at = now()
                  WHERE id = $1 AND status = 'pending'",
            )
            .bind(w.id)
            .bind(&reason)
            .execute(&mut *tx)
            .await?
            .rows_affected();

            if updated > 0 {
                sqlx::query(
                    "UPDATE balances
                        SET available = available + $2, updated_at = now()
                      WHERE merchant_id = $1 AND asset = $3",
                )
                .bind(w.merchant_id)
                .bind(w.amount_stroops)
                .bind(&w.asset)
                .execute(&mut *tx)
                .await?;

                tx.commit().await?;
                tracing::info!(
                    withdrawal_id = %w.id,
                    merchant_id = %w.merchant_id,
                    "pending withdrawal not found on provider, marked as failed and refunded balance"
                );
            } else {
                tx.rollback().await?;
            }

            Ok(ReconciledStatus::FailedAndRefunded)
        }
    }
}

pub async fn reconcile_pending_withdrawals_with_age(
    db: &PgPool,
    provider: &dyn PaymentProvider,
    older_than: chrono::Duration,
) -> Result<ReconciliationReport, sqlx::Error> {
    let pending_list = find_pending_withdrawals_older_than(db, older_than).await?;
    let mut report = ReconciliationReport {
        total_scanned: pending_list.len(),
        ..Default::default()
    };

    if pending_list.is_empty() {
        tracing::debug!("no pending withdrawals older than {:?} to reconcile", older_than);
        return Ok(report);
    }

    tracing::info!(
        count = pending_list.len(),
        "starting reconciliation of pending withdrawals"
    );

    for w in &pending_list {
        match reconcile_single_withdrawal(db, provider, w).await {
            Ok(ReconciledStatus::Completed) => report.completed += 1,
            Ok(ReconciledStatus::Processing) => report.processing += 1,
            Ok(ReconciledStatus::Pending) => report.pending += 1,
            Ok(ReconciledStatus::FailedAndRefunded) => report.failed_and_refunded += 1,
            Err(err) => {
                report.errors += 1;
                tracing::warn!(
                    withdrawal_id = %w.id,
                    error = %err,
                    "failed to reconcile pending withdrawal"
                );
            }
        }
    }

    tracing::info!(
        total = report.total_scanned,
        completed = report.completed,
        processing = report.processing,
        pending = report.pending,
        failed_and_refunded = report.failed_and_refunded,
        errors = report.errors,
        "completed pending withdrawals reconciliation"
    );

    Ok(report)
}

pub async fn reconcile_pending_withdrawals(
    db: &PgPool,
    provider: &dyn PaymentProvider,
) -> Result<ReconciliationReport, sqlx::Error> {
    reconcile_pending_withdrawals_with_age(db, provider, chrono::Duration::minutes(10)).await
}
