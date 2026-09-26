use sqlx::PgPool;

use crate::auth::password;
use crate::models::{Merchant, User};

/// Number of consecutive failed password attempts before an account is locked.
const MAX_FAILED_LOGIN_ATTEMPTS: i32 = 10;
/// How long an account stays locked after exceeding the failed attempt limit.
const LOCKOUT_DURATION_MINUTES: i64 = 30;

#[derive(Debug, thiserror::Error)]
pub enum UserError {
    #[error("invalid email or password")]
    InvalidCredentials,
    #[error("account locked due to too many failed login attempts; try again later")]
    AccountLocked,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Materializes a brand-new account. The only caller is
/// `services::otp::verify`, once a signup's OTP challenge passes — never
/// called speculatively, so an unverified phone can never end up attached
/// to a real, loggable-in account. `password_hash` must already be hashed.
pub async fn create_verified(
    db: &PgPool,
    email: &str,
    password_hash: &str,
    name: &str,
    phone_number: &str,
) -> Result<(User, Merchant), sqlx::Error> {
    let mut tx = db.begin().await?;
    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (email, password_hash, name, phone_number, phone_verified)
         VALUES ($1, $2, $3, $4, true)
         RETURNING id, email, password_hash, name, is_admin, phone_number, phone_verified, created_at, updated_at",
    )
    .bind(email)
    .bind(password_hash)
    .bind(name)
    .bind(phone_number)
    .fetch_one(&mut *tx)
    .await?;

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

/// Which unique constraint a `23505` violation hit, if any — lets a caller
/// distinguish "email taken" from "phone taken" from an unrelated conflict.
pub(crate) fn unique_violation_field(err: &sqlx::Error) -> Option<&str> {
    match err {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => db.constraint(),
        _ => None,
    }
}

pub async fn login(db: &PgPool, email: &str, password_raw: &str) -> Result<(User, Option<Merchant>), UserError> {
    let user = sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash, name, is_admin, phone_number, phone_verified, created_at, updated_at
           FROM users WHERE email = $1",
    )
    .bind(email)
    .fetch_optional(db)
    .await?
    .ok_or(UserError::InvalidCredentials)?;

    // Reject attempts against an account that is still locked. A lock whose
    // `locked_until` has elapsed is treated as expired and cleared below.
    let locked: bool = sqlx::query_scalar(
        "SELECT locked_until IS NOT NULL AND locked_until > now() FROM users WHERE id = $1",
    )
    .bind(user.id)
    .fetch_one(db)
    .await?;

    if locked {
        return Err(UserError::AccountLocked);
    }

    if !password::verify(password_raw, &user.password_hash) {
        // Increment the consecutive-failure counter and lock the account once
        // the threshold is reached. A successful login resets the counter.
        sqlx::query(
            "UPDATE users
                SET failed_login_count = failed_login_count + 1,
                    locked_until = CASE
                        WHEN failed_login_count + 1 >= $2
                        THEN now() + make_interval(mins => $3)
                        ELSE locked_until
                    END,
                    updated_at = now()
              WHERE id = $1",
        )
        .bind(user.id)
        .bind(MAX_FAILED_LOGIN_ATTEMPTS)
        .bind(LOCKOUT_DURATION_MINUTES as i32)
        .execute(db)
        .await?;

        return Err(UserError::InvalidCredentials);
    }

    // Successful authentication: clear any accumulated failures and expired lock.
    sqlx::query(
        "UPDATE users
            SET failed_login_count = 0, locked_until = NULL, updated_at = now()
          WHERE id = $1 AND (failed_login_count <> 0 OR locked_until IS NOT NULL)",
    )
    .bind(user.id)
    .execute(db)
    .await?;

    let merchant = sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at FROM merchants WHERE user_id = $1 LIMIT 1",
    )
    .bind(user.id)
    .fetch_optional(db)
    .await?;

    Ok((user, merchant))
}

/// Clears the lockout state for an account so it can log in again. Intended
/// for the admin unlock endpoint; returns `true` when a user row was updated.
pub async fn unlock_account(db: &PgPool, user_id: uuid::Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE users
            SET failed_login_count = 0, locked_until = NULL, updated_at = now()
          WHERE id = $1",
    )
    .bind(user_id)
    .execute(db)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn user_by_id(db: &PgPool, user_id: uuid::Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash, name, is_admin, phone_number, phone_verified, created_at, updated_at
           FROM users WHERE id = $1",
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
