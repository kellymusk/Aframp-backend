use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

use crate::auth::password;
use crate::models::{Merchant, User};

/// Consecutive failed login attempts before an account is locked.
const MAX_FAILED_ATTEMPTS: i32 = 10;
/// How long an account stays locked once the threshold is reached.
const LOCKOUT_SECS: i64 = 30 * 60; // 30 minutes

#[derive(Debug, thiserror::Error)]
pub enum UserError {
    #[error("email already registered")]
    EmailTaken,
    #[error("invalid email or password")]
    InvalidCredentials,
    #[error("account temporarily locked")]
    AccountLocked(DateTime<Utc>),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("password hashing failed")]
    Hash,
}

pub async fn signup(
    db: &PgPool,
    email: &str,
    password_raw: &str,
    name: &str,
) -> Result<(User, Merchant), UserError> {
    let password_hash = password::hash(password_raw).map_err(|_| UserError::Hash)?;

    let mut tx = db.begin().await?;
    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (email, password_hash, name)
         VALUES ($1, $2, $3)
         RETURNING id, email, password_hash, name, created_at, updated_at",
    )
    .bind(email)
    .bind(&password_hash)
    .bind(name)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_insert_error)?;

    let merchant = sqlx::query_as::<_, Merchant>(
        "INSERT INTO merchants (user_id, name)
         VALUES ($1, $2)
         RETURNING id, user_id, name, created_at",
    )
    .bind(user.id)
    .bind(name)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok((user, merchant))
}

fn map_insert_error(err: sqlx::Error) -> UserError {
    if is_unique_violation(&err) {
        UserError::EmailTaken
    } else {
        UserError::Database(err)
    }
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(
        err,
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505")
    )
}

pub async fn login(db: &PgPool, email: &str, password_raw: &str) -> Result<(User, Option<Merchant>), UserError> {
    // Reject outright if the account is currently locked.
    if let Some(locked_until) = get_locked_until(db, email).await? {
        return Err(UserError::AccountLocked(locked_until));
    }

    let user = sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash, name, created_at, updated_at FROM users WHERE email = $1",
    )
    .bind(email)
    .fetch_optional(db)
    .await?
    .ok_or(UserError::InvalidCredentials)?;

    if !password::verify(password_raw, &user.password_hash) {
        // Record the failure and lock the account if the threshold is reached.
        if record_failed_attempt(db, email).await? {
            let locked_until = get_locked_until(db, email).await?.unwrap_or_else(|| {
                Utc::now() + Duration::seconds(LOCKOUT_SECS)
            });
            return Err(UserError::AccountLocked(locked_until));
        }
        return Err(UserError::InvalidCredentials);
    }

    // Successful login clears any prior failed-attempt history.
    clear_failed_attempts(db, email).await?;

    let merchant = sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at FROM merchants WHERE user_id = $1 LIMIT 1",
    )
    .bind(user.id)
    .fetch_optional(db)
    .await?;

    Ok((user, merchant))
}

/// Returns the lock-expiry instant if the account is currently locked, or `None`
/// when there is no active lock (no record, or a lock that has already elapsed).
async fn get_locked_until(db: &PgPool, email: &str) -> Result<Option<DateTime<Utc>>, UserError> {
    let row: Option<(Option<DateTime<Utc>>,)> =
        sqlx::query_as("SELECT locked_until FROM login_attempts WHERE email = $1")
            .bind(email)
            .fetch_optional(db)
            .await?;

    Ok(row
        .and_then(|(locked_until,)| locked_until)
        .filter(|until| *until > Utc::now()))
}

/// Increments the failed-attempt counter for `email`, locking the account
/// (setting `locked_until`) once it reaches the threshold. Returns `true` when
/// the account is now locked.
async fn record_failed_attempt(db: &PgPool, email: &str) -> Result<bool, UserError> {
    let row: (i32,) = sqlx::query_as(
        "INSERT INTO login_attempts (email, failed_attempts, updated_at)
             VALUES ($1, 1, now())
         ON CONFLICT (email) DO UPDATE
             SET failed_attempts = login_attempts.failed_attempts + 1,
                 updated_at = now()
         RETURNING failed_attempts",
    )
    .bind(email)
    .fetch_one(db)
    .await?;

    if row.0 >= MAX_FAILED_ATTEMPTS {
        let locked_until = Utc::now() + Duration::seconds(LOCKOUT_SECS);
        sqlx::query("UPDATE login_attempts SET locked_until = $2 WHERE email = $1")
            .bind(email)
            .bind(locked_until)
            .execute(db)
            .await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Resets the failed-attempt counter and clears any lock for `email`.
async fn clear_failed_attempts(db: &PgPool, email: &str) -> Result<(), UserError> {
    sqlx::query(
        "UPDATE login_attempts SET failed_attempts = 0, locked_until = NULL WHERE email = $1",
    )
    .bind(email)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn user_by_id(db: &PgPool, user_id: uuid::Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash, name, created_at, updated_at FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn merchant_by_user(db: &PgPool, user_id: uuid::Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at FROM merchants WHERE user_id = $1 LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn merchant_by_id(db: &PgPool, merchant_id: uuid::Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at FROM merchants WHERE id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}
