use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sqlx::PgPool;
use uuid::Uuid;

use crate::models::status::WithdrawalStatus;
use crate::models::{NewWithdrawal, Withdrawal};
use crate::payments::{PaymentProvider, PayoutRequest, PayoutResult, PayoutVerification};

/// 1 unit of a Stellar asset = 10,000,000 stroops; 1 Naira = 100 kobo.
/// cNGN is pegged 1:1 to NGN, so 1 kobo = 100,000 stroops.
const STROOPS_PER_KOBO: i64 = 100_000;

/// How many times to try recording a successful payout before giving up.
const PAYOUT_RECORD_ATTEMPTS: u32 = 3;
/// How many times to try committing the refund for a failed payout.
const REFUND_ATTEMPTS: u32 = 3;
/// How long a resolved account name stays valid in the in-process cache.
const VERIFICATION_TTL: Duration = Duration::from_secs(600);
/// Paystack flat fee in kobo (₦100 = 10,000 kobo)
const PAYSTACK_FLAT_FEE_KOBO: i64 = 10_000;

/// Paystack flat fee in stroops
const PAYSTACK_FEE_STROOPS: i64 = PAYSTACK_FLAT_FEE_KOBO * STROOPS_PER_KOBO;

#[derive(Debug, thiserror::Error)]
pub enum WithdrawalError {
    #[error("insufficient available balance")]
    InsufficientBalance,
    #[error("withdrawals are only supported for the cNGN asset")]
    UnsupportedAsset,
    #[error("amount_stroops must be a whole number of kobo (a multiple of {STROOPS_PER_KOBO})")]
    InvalidAmountPrecision,
    #[error("bank account could not be verified: {0}")]
    AccountVerificationFailed(String),
    #[error("daily withdrawal limit exceeded")]
    DailyLimitExceeded,
    #[error("payout provider failed: {0}")]
    PayoutFailed(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// A bank account whose ownership has been confirmed by the payout provider.
#[derive(Debug, Clone)]
pub struct VerifiedAccount {
    pub account_number: String,
    pub bank_code: String,
    pub account_name: String,
}

struct CacheEntry {
    account_name: String,
    expires_at: Instant,
}

/// Process-local cache of resolved account names, keyed by `(bank_code,
/// account_number)`. Avoids hammering the provider's resolve endpoint for the
/// same account across repeated withdrawal attempts.
static VERIFIED_ACCOUNTS: Mutex<Option<HashMap<(String, String), CacheEntry>>> = Mutex::new(None);

fn cache_key(bank_code: &str, account_number: &str) -> (String, String) {
    (bank_code.to_string(), account_number.to_string())
}

fn cache_lookup(bank_code: &str, account_number: &str) -> Option<String> {
    let mut guard = VERIFIED_ACCOUNTS.lock().ok()?;
    let map = guard.as_mut()?;
    let key = cache_key(bank_code, account_number);
    match map.get(&key) {
        Some(entry) if entry.expires_at > Instant::now() => Some(entry.account_name.clone()),
        Some(_) => {
            map.remove(&key);
            None
        }
        None => None,
    }
}

fn cache_store(bank_code: &str, account_number: &str, account_name: &str) {
    if let Ok(mut guard) = VERIFIED_ACCOUNTS.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        map.insert(
            cache_key(bank_code, account_number),
            CacheEntry {
                account_name: account_name.to_string(),
                expires_at: Instant::now() + VERIFICATION_TTL,
            },
        );
    }
}

/// Resolve a bank account to its registered account name, caching successful
/// resolutions so repeated calls for the same account skip the provider.
///
/// This is the hard gate used before any withdrawal is attempted: a failure
/// here means the account could not be verified and the caller must not
/// proceed to move money.
pub async fn verify_bank_account(
    provider: &dyn PaymentProvider,
    bank_code: &str,
    account_number: &str,
) -> Result<VerifiedAccount, WithdrawalError> {
    if let Some(account_name) = cache_lookup(bank_code, account_number) {
        return Ok(VerifiedAccount {
            account_number: account_number.to_string(),
            bank_code: bank_code.to_string(),
            account_name,
        });
    }

    let account_name = provider
        .resolve_account(bank_code, account_number)
        .await
        .map_err(WithdrawalError::AccountVerificationFailed)?;

    cache_store(bank_code, account_number, &account_name);

    Ok(VerifiedAccount {
        account_number: account_number.to_string(),
        bank_code: bank_code.to_string(),
        account_name,
    })
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

/// Calculate withdrawal fee and net amount.
/// Returns (fee_stroops, net_amount_stroops).
/// Fee is Paystack's flat ₦100 fee.
pub fn calculate_withdrawal_fee(amount_stroops: i64) -> (i64, i64) {
    let fee = PAYSTACK_FEE_STROOPS;
    let net = amount_stroops.saturating_sub(fee).max(0);
    (fee, net)
}

pub async fn create_withdrawal(
    db: &PgPool,
    provider: &dyn PaymentProvider,
    withdrawal: NewWithdrawal,
    daily_limit_stroops: Option<i64>,
) -> Result<Withdrawal, WithdrawalError> {
    create_withdrawal_idempotent(db, provider, withdrawal, None, daily_limit_stroops).await
}

/// Create a withdrawal, optionally guarded by an idempotency key.
///
/// When `idempotency_key` is `Some`, a retry that reuses the same key for the
/// same merchant returns the previously created withdrawal instead of
/// initiating a second payout. The key is persisted on the row so the lookup
/// survives process restarts and concurrent retries.
pub async fn create_withdrawal_idempotent(
    db: &PgPool,
    provider: &dyn PaymentProvider,
    withdrawal: NewWithdrawal,
    idempotency_key: Option<&str>,
    daily_limit_stroops: Option<i64>,
) -> Result<Withdrawal, WithdrawalError> {
    let idempotency_key = idempotency_key.or(withdrawal.idempotency_key.as_deref());
    if let Some(key) = idempotency_key {
        if let Some(existing) = find_by_idempotency_key(db, withdrawal.merchant_id, key).await? {
            return Ok(existing);
        }
    }

    if withdrawal.asset != "cNGN" {
        return Err(WithdrawalError::UnsupportedAsset);
    }
    if withdrawal.amount_stroops % STROOPS_PER_KOBO != 0 {
        return Err(WithdrawalError::InvalidAmountPrecision);
    }
    let amount_kobo = withdrawal.amount_stroops / STROOPS_PER_KOBO;

    // Hard requirement: resolve the account *before* debiting the balance or
    // creating the withdrawal row. A failed resolution aborts here, so no
    // funds are ever moved against an unverified account.
    verify_bank_account(provider, &withdrawal.bank_code, &withdrawal.account_number).await?;

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

    // Checked after the debit so the balance row lock serializes concurrent
    // withdrawals for this merchant — two requests can't both slip under it.
    if let Some(limit) = daily_limit_stroops {
        let today: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount_stroops), 0)::bigint
               FROM withdrawals
              WHERE merchant_id = $1
                AND status <> 'failed'
                AND created_at >= date_trunc('day', now(), 'UTC')",
        )
        .bind(withdrawal.merchant_id)
        .fetch_one(&mut *tx)
        .await?;

        if today + withdrawal.amount_stroops > limit {
            tx.rollback().await?;
            return Err(WithdrawalError::DailyLimitExceeded);
        }
    }

    let w = sqlx::query_as::<_, Withdrawal>(
        "INSERT INTO withdrawals (
             merchant_id, amount_stroops, asset, status, bank_code, account_number,
             idempotency_key
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (merchant_id, idempotency_key) WHERE idempotency_key IS NOT NULL
         DO NOTHING
         RETURNING id, merchant_id, amount_stroops, asset, status, provider,
                   provider_reference, bank_code, account_number, failure_reason,
                   idempotency_key, created_at, updated_at",
    )
    .bind(withdrawal.merchant_id)
    .bind(withdrawal.amount_stroops)
    .bind(&withdrawal.asset)
    .bind(WithdrawalStatus::Pending)
    .bind(&withdrawal.bank_code)
    .bind(&withdrawal.account_number)
    .bind(idempotency_key)
    .fetch_optional(&mut *tx)
    .await?;

    // A concurrent retry with the same key won the insert race. Undo the debit
    // we just made and return the row the other request created.
    let w = match w {
        Some(w) => w,
        None => {
            tx.rollback().await?;
            let key = idempotency_key.expect("conflict only possible with a key");
            return find_by_idempotency_key(db, withdrawal.merchant_id, key)
                .await?
                .ok_or(WithdrawalError::InsufficientBalance);
        }
    };

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
            // The transfer has already gone out, so a failed write here must not
            // be swallowed: retry transient DB errors, and if they persist, log
            // enough for an operator to reconcile the row by hand.
            let mut attempt = 0;
            loop {
                attempt += 1;
                match record_payout(db, w.id, &result).await {
                    Ok(updated) => return Ok(updated),
                    Err(e) if attempt < PAYOUT_RECORD_ATTEMPTS => {
                        tracing::warn!(
                            withdrawal_id = %w.id,
                            attempt,
                            error = %e,
                            "retrying payout status write"
                        );
                        tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
                    }
                    Err(e) => {
                        tracing::error!(
                            withdrawal_id = %w.id,
                            transfer_code = %result.provider_reference,
                            provider = %result.provider,
                            error = %e,
                            "payout succeeded but recording it failed; withdrawal needs manual reconciliation"
                        );
                        return Err(WithdrawalError::Database(e));
                    }
                }
            }
        }
        Err(err) => {
            // Refund + mark failed as one atomic unit, in a fresh transaction —
            // the original debit is already committed, so this is a compensating
            // action, not a rollback. Keeps an audit trail instead of pretending
            // the attempt never happened.
            let mut attempt = 0;
            loop {
                attempt += 1;
                match refund_failed_payout(db, &w, &err).await {
                    Ok(()) => break,
                    Err(e) if attempt < REFUND_ATTEMPTS => {
                        tracing::warn!(withdrawal_id = %w.id, attempt, error = %e, "retrying refund after failed payout");
                        tokio::time::sleep(Duration::from_millis(100 * 2u64.pow(attempt - 1))).await;
                    }
                    Err(e) => {
                        tracing::error!(
                            withdrawal_id = %w.id,
                            merchant_id = %w.merchant_id,
                            error = %e,
                            "payout failed and the refund could not be committed; balance needs manual reconciliation"
                        );
                        return Err(WithdrawalError::Database(e));
                    }
                }
            }
            Err(WithdrawalError::PayoutFailed(err))
        }
    }
}

/// Refund a failed payout and mark the withdrawal failed, atomically.
/// Idempotent, so it is safe to retry after an ambiguous commit failure:
/// the balance is only credited when this call moves the row out of a
/// non-failed state.
async fn refund_failed_payout(db: &PgPool, w: &Withdrawal, reason: &str) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    let flipped = sqlx::query(
        "UPDATE withdrawals
            SET status = $2, failure_reason = $3, updated_at = now()
          WHERE id = $1 AND status <> $2",
    )
    .bind(w.id)
    .bind(WithdrawalStatus::Failed)
    .bind(reason)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if flipped == 1 {
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
    }
    tx.commit().await
}

async fn record_payout(
    db: &PgPool,
    id: Uuid,
    result: &PayoutResult,
) -> Result<Withdrawal, sqlx::Error> {
    sqlx::query_as::<_, Withdrawal>(
        "UPDATE withdrawals
            SET provider = $2, provider_reference = $3, status = $4, updated_at = now()
          WHERE id = $1
          RETURNING id, merchant_id, amount_stroops, asset, status, provider,
                    provider_reference, bank_code, account_number, failure_reason,
                    idempotency_key, created_at, updated_at",
    )
    .bind(id)
    .bind(&result.provider)
    .bind(&result.provider_reference)
    .bind(&result.status)
    .fetch_one(db)
    .await
}

/// Total of a merchant's withdrawals today (UTC) that count toward the daily
/// limit: everything except failed (refunded) attempts.
pub async fn withdrawals_today_stroops(
    db: &PgPool,
    merchant_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(amount_stroops), 0)
           FROM withdrawals
          WHERE merchant_id = $1
            AND status <> 'failed'
            AND created_at >= date_trunc('day', now(), 'UTC')",
    )
    .bind(merchant_id)
    .fetch_one(db)
    .await
}

/// Look up a withdrawal previously created with the given idempotency key for
/// this merchant. Returns `None` when the key has not been used yet.
pub async fn find_by_idempotency_key(
    db: &PgPool,
    merchant_id: Uuid,
    idempotency_key: &str,
) -> Result<Option<Withdrawal>, sqlx::Error> {
    sqlx::query_as::<_, Withdrawal>(
        "SELECT id, merchant_id, amount_stroops, asset, status, provider,
                provider_reference, bank_code, account_number, failure_reason,
                idempotency_key, created_at, updated_at
           FROM withdrawals
          WHERE merchant_id = $1 AND idempotency_key = $2",
    )
    .bind(merchant_id)
    .bind(idempotency_key)
    .fetch_optional(db)
    .await
}

/// Reconcile a withdrawal from an asynchronous Paystack transfer webhook.
///
/// Paystack transfer events (`transfer.success`, `transfer.failed`,
/// `transfer.reversed`) arrive after the initial payout call, which in live
/// mode only returns a `pending` transfer. This applies the terminal status to
/// the matching withdrawal and, on failure or reversal, refunds the merchant's
/// available balance exactly once.
///
/// The update is guarded so it is idempotent: a withdrawal that is already in a
/// terminal state is left untouched, and the balance refund only happens on the
/// transition out of `pending`. This makes duplicate webhook deliveries safe.
///
/// The withdrawal is matched by our own reference (its id, which we send to
/// Paystack as the transfer reference) or, failing that, the transfer code.
/// When `event` is given, the raw payload is recorded in `webhook_events`;
/// a repeat delivery of an already-recorded event changes nothing.
pub async fn reconcile_withdrawal_status(
    db: &PgPool,
    reference: Option<&str>,
    provider_reference: Option<&str>,
    status: &str,
    failure_reason: Option<&str>,
    event: Option<(&str, &serde_json::Value)>,
) -> Result<Option<Withdrawal>, sqlx::Error> {
    let mut tx = db.begin().await?;
    let reference_id = reference.and_then(|r| Uuid::parse_str(r).ok());

    // Lock the row and read its current state so we can decide whether a refund
    // is owed. `FOR UPDATE` serialises concurrent webhook deliveries for the
    // same withdrawal.
    let current = sqlx::query_as::<_, Withdrawal>(
        "SELECT id, merchant_id, amount_stroops, asset, status, provider,
                provider_reference, bank_code, account_number, failure_reason,
                idempotency_key, created_at, updated_at
           FROM withdrawals
          WHERE id = $1 OR ($1 IS NULL AND provider_reference = $2)
          FOR UPDATE",
    )
    .bind(reference_id)
    .bind(provider_reference)
    .fetch_optional(&mut *tx)
    .await?;

    let current = match current {
        Some(w) => w,
        None => {
            tx.rollback().await?;
            return Ok(None);
        }
    };

    if let Some((external_id, payload)) = event {
        let recorded = sqlx::query(
            "INSERT INTO webhook_events (merchant_id, provider, external_id, payload)
             VALUES ($1, 'paystack', $2, $3)
             ON CONFLICT (provider, external_id) DO NOTHING",
        )
        .bind(current.merchant_id)
        .bind(external_id)
        .bind(payload)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if recorded == 0 {
            // Already processed this delivery.
            tx.rollback().await?;
            return Ok(Some(current));
        }
    }

    // Already terminal: nothing to do. Keeps duplicate deliveries idempotent.
    if current.status != WithdrawalStatus::Pending {
        tx.rollback().await?;
        return Ok(Some(current));
    }

    let refund = matches!(status, "failed" | "reversed");

    if refund {
        sqlx::query(
            "UPDATE balances
                SET available = available + $2, updated_at = now()
              WHERE merchant_id = $1 AND asset = $3",
        )
        .bind(current.merchant_id)
        .bind(current.amount_stroops)
        .bind(&current.asset)
        .execute(&mut *tx)
        .await?;
    }

    let updated = sqlx::query_as::<_, Withdrawal>(
        "UPDATE withdrawals
            SET status = $2, failure_reason = $3, updated_at = now()
          WHERE id = $1
          RETURNING id, merchant_id, amount_stroops, asset, status, provider,
                    provider_reference, bank_code, account_number, failure_reason,
                    idempotency_key, created_at, updated_at",
    )
    .bind(current.id)
    .bind(status)
    .bind(failure_reason)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Some(updated))
}

/// Keyset-paginated withdrawals, newest first. Fetches `limit + 1` rows so
/// `Page::new` can tell whether another page follows; pass the previous
/// page's last `(created_at, id)` as `cursor` to continue. Rows inserted while
/// a client is paging land before the cursor and can't shift later pages.
pub async fn withdrawals_by_merchant_cursor(
    db: &PgPool,
    merchant_id: Uuid,
    limit: i64,
    cursor: Option<crate::pagination::Cursor>,
) -> Result<Vec<Withdrawal>, sqlx::Error> {
    sqlx::query_as::<_, Withdrawal>(
        "SELECT id, merchant_id, amount_stroops, asset, status, provider,
                provider_reference, bank_code, account_number, failure_reason,
                idempotency_key, created_at, updated_at
           FROM withdrawals
          WHERE merchant_id = $1
            AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3))
          ORDER BY created_at DESC, id DESC
          LIMIT $4",
    )
    .bind(merchant_id)
    .bind(cursor.map(|c| c.created_at))
    .bind(cursor.map(|c| c.id))
    .bind(limit + 1)
    .fetch_all(db)
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
                idempotency_key, created_at, updated_at
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
                idempotency_key, created_at, updated_at
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
