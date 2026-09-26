use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{NewWithdrawal, Withdrawal};
use crate::payments::{PaymentProvider, PayoutRequest};

/// 1 unit of a Stellar asset = 10,000,000 stroops; 1 Naira = 100 kobo.
/// cNGN is pegged 1:1 to NGN, so 1 kobo = 100,000 stroops.
const STROOPS_PER_KOBO: i64 = 100_000;

/// How long a resolved account name stays valid in the in-process cache.
const VERIFICATION_TTL: Duration = Duration::from_secs(600);

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

pub async fn create_withdrawal(
    db: &PgPool,
    provider: &dyn PaymentProvider,
    withdrawal: NewWithdrawal,
) -> Result<Withdrawal, WithdrawalError> {
    create_withdrawal_idempotent(db, provider, withdrawal, None).await
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
) -> Result<Withdrawal, WithdrawalError> {
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
    let verified = verify_bank_account(
        provider,
        &withdrawal.bank_code,
        &withdrawal.account_number,
    )
    .await?;

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
             merchant_id, amount_stroops, asset, status, bank_code, account_number,
             idempotency_key
         )
         VALUES ($1, $2, $3, 'pending', $4, $5, $6)
         ON CONFLICT (merchant_id, idempotency_key) DO NOTHING
         RETURNING id, merchant_id, amount_stroops, asset, status, provider,
                   provider_reference, bank_code, account_number, failure_reason,
                   idempotency_key, created_at, updated_at",
    )
    .bind(withdrawal.merchant_id)
    .bind(withdrawal.amount_stroops)
    .bind(&withdrawal.asset)
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
            account_name: verified.account_name.clone(),
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
                            idempotency_key, created_at, updated_at",
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
pub async fn reconcile_withdrawal_status(
    db: &PgPool,
    provider_reference: &str,
    status: &str,
    failure_reason: Option<&str>,
) -> Result<Option<Withdrawal>, sqlx::Error> {
    let mut tx = db.begin().await?;

    // Lock the row and read its current state so we can decide whether a refund
    // is owed. `FOR UPDATE` serialises concurrent webhook deliveries for the
    // same withdrawal.
    let current = sqlx::query_as::<_, Withdrawal>(
        "SELECT id, merchant_id, amount_stroops, asset, status, provider,
                provider_reference, bank_code, account_number, failure_reason,
                idempotency_key, created_at, updated_at
           FROM withdrawals
          WHERE provider_reference = $1
          FOR UPDATE",
    )
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

    // Already terminal: nothing to do. Keeps duplicate deliveries idempotent.
    if current.status != "pending" {
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
