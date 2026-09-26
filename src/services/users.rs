use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::password;
use crate::models::{Merchant, User};

/// Number of consecutive failed login attempts before an account is locked.
const MAX_FAILED_ATTEMPTS: i32 = 10;
/// How long (minutes) an account stays locked after hitting the threshold.
const LOCKOUT_DURATION_MINS: i64 = 30;

/// Column list shared by all SELECT queries on the users table — keeps the
/// new lockout columns in sync across every call site.
const USER_COLS: &str =
    "id, email, password_hash, name, is_admin, phone_number, phone_verified, \
     failed_login_count, locked_until, created_at, updated_at";

#[derive(Debug, thiserror::Error)]
pub enum UserError {
    #[error("invalid email or password")]
    InvalidCredentials,
    /// The account is temporarily locked due to too many failed attempts.
    /// `until` is when the lock expires (30 minutes from the 10th failure).
    #[error("account locked until {until}")]
    AccountLocked { until: chrono::DateTime<Utc> },
    /// The merchant account has been suspended by an admin.
    #[error("merchant account has been suspended")]
    MerchantSuspended,
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
        &format!(
            "INSERT INTO users (email, password_hash, name, phone_number, phone_verified)
             VALUES ($1, $2, $3, $4, true)
             RETURNING {USER_COLS}"
        ),
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
         RETURNING id, user_id, name, suspended_at, created_at",
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

/// Authenticate a user by email + password.
///
/// On success: resets `failed_login_count` to 0.
/// On wrong password: increments `failed_login_count`; locks the account for
/// [`LOCKOUT_DURATION_MINS`] minutes after [`MAX_FAILED_ATTEMPTS`] failures.
/// Returns [`UserError::AccountLocked`] when the account is currently locked,
/// regardless of whether the supplied password is correct.
pub async fn login(db: &PgPool, email: &str, password_raw: &str) -> Result<(User, Option<Merchant>), UserError> {
    let user = sqlx::query_as::<_, User>(
        &format!(
            "SELECT {USER_COLS} FROM users WHERE email = $1"
        ),
    )
    .bind(email)
    .fetch_optional(db)
    .await?
    .ok_or(UserError::InvalidCredentials)?;

    // Check lockout before verifying the password — avoids leaking timing info
    // about whether a locked account's password is correct or not.
    if let Some(locked_until) = user.locked_until {
        if Utc::now() < locked_until {
            return Err(UserError::AccountLocked { until: locked_until });
        }
    }

    if !password::verify(password_raw, &user.password_hash) {
        // Increment failure counter; lock if threshold reached.
        let new_count = user.failed_login_count + 1;
        if new_count >= MAX_FAILED_ATTEMPTS {
            let locked_until = Utc::now() + chrono::Duration::minutes(LOCKOUT_DURATION_MINS);
            sqlx::query(
                "UPDATE users
                    SET failed_login_count = $2, locked_until = $3, updated_at = now()
                  WHERE id = $1",
            )
            .bind(user.id)
            .bind(new_count)
            .bind(locked_until)
            .execute(db)
            .await?;
        } else {
            sqlx::query(
                "UPDATE users
                    SET failed_login_count = $2, updated_at = now()
                  WHERE id = $1",
            )
            .bind(user.id)
            .bind(new_count)
            .execute(db)
            .await?;
        }
        return Err(UserError::InvalidCredentials);
    }

    // Successful authentication — reset the failure counter and clear any
    // expired lock (a time-expired lock doesn't block login above, but we
    // clean it up here so the column doesn't mislead in admin views).
    sqlx::query(
        "UPDATE users
            SET failed_login_count = 0, locked_until = NULL, updated_at = now()
          WHERE id = $1",
    )
    .bind(user.id)
    .execute(db)
    .await?;

    let merchant = sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, suspended_at, created_at FROM merchants WHERE user_id = $1 LIMIT 1",
    )
    .bind(user.id)
    .fetch_optional(db)
    .await?;

    // Refuse login for suspended merchants.
    if let Some(ref m) = merchant {
        if m.is_suspended() {
            return Err(UserError::MerchantSuspended);
        }
    }

    Ok((user, merchant))
}

pub async fn user_by_id(db: &PgPool, user_id: Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(
        &format!("SELECT {USER_COLS} FROM users WHERE id = $1"),
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn merchant_by_user(db: &PgPool, user_id: Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, suspended_at, created_at FROM merchants WHERE user_id = $1 LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn merchant_by_id(db: &PgPool, merchant_id: Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, suspended_at, created_at FROM merchants WHERE id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}

/// Admin operation: clear a user's account lockout immediately.
/// Also resets `failed_login_count` so the next bad attempt starts fresh.
pub async fn admin_unlock(db: &PgPool, user_id: Uuid) -> Result<bool, sqlx::Error> {
    let rows = sqlx::query(
        "UPDATE users
            SET failed_login_count = 0, locked_until = NULL, updated_at = now()
          WHERE id = $1",
    )
    .bind(user_id)
    .execute(db)
    .await?
    .rows_affected();
    Ok(rows > 0)
}
