use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::password;
use crate::models::{Merchant, User, UserProfile};

/// Number of consecutive failed login attempts before an account is locked.
const MAX_FAILED_ATTEMPTS: i32 = 10;
/// How long (minutes) an account stays locked after hitting the threshold.
const LOCKOUT_DURATION_MINS: i64 = 30;

/// Column list shared by all SELECT queries on the users table — keeps the
/// new lockout columns in sync across every call site.
pub(crate) const USER_COLS: &str =
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
    let (user, merchant) = create_verified_in_tx(&mut tx, email, password_hash, name, phone_number).await?;
    tx.commit().await?;
    Ok((user, merchant))
}

/// Same as [`create_verified`] but runs on a caller-supplied transaction so
/// the account creation can be committed atomically together with other
/// work (e.g. claiming the OTP challenge). The caller owns the commit.
pub async fn create_verified_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    email: &str,
    password_hash: &str,
    name: &str,
    phone_number: &str,
) -> Result<(User, Merchant), sqlx::Error> {
    let user = sqlx::query_as::<_, User>(&format!(
        "INSERT INTO users (email, password_hash, name, phone_number, phone_verified)
             VALUES ($1, $2, $3, $4, true)
             RETURNING {USER_COLS}"
    ))
    .bind(email)
    .bind(password_hash)
    .bind(name)
    .bind(phone_number)
    .fetch_one(&mut **tx)
    .await?;

    let merchant = sqlx::query_as::<_, Merchant>(
        "INSERT INTO merchants (user_id, name)
         VALUES ($1, $2)
         RETURNING id, user_id, name, suspended_at, created_at, updated_at",
    )
    .bind(user.id)
    .bind(name)
    .fetch_one(&mut **tx)
    .await?;

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
/// If a previous lock has expired, the counter is reset to 0 first so the
/// merchant gets a fresh [`MAX_FAILED_ATTEMPTS`]-attempt window.
/// Returns [`UserError::AccountLocked`] when the account is currently locked,
/// regardless of whether the supplied password is correct.
#[tracing::instrument(skip_all, err)]
pub async fn login(
    db: &PgPool,
    email: &str,
    password_raw: &str,
) -> Result<(User, Option<Merchant>), UserError> {
    let user = sqlx::query_as::<_, User>(&format!(
        "SELECT {USER_COLS} FROM users WHERE email = $1 AND deleted_at IS NULL"
    ))
    .bind(email)
    .fetch_optional(db)
    .await?;

    // Equalize response time between the "unknown email" and "wrong password"
    // paths: when no user is found we still run a full Argon2 verification
    // against a dummy hash (same parameters as real hashes) so the timing of
    // the two failure modes is indistinguishable. This closes the username
    // enumeration timing side-channel.
    let user = match user {
        Some(user) => user,
        None => {
            password::verify(password_raw, password::dummy_hash());
            return Err(UserError::InvalidCredentials);
        }
    };

    // Check lockout before verifying the password — avoids leaking timing info
    // about whether a locked account's password is correct or not. If the lock
    // is still active we bail out; if it has expired we fall through and treat
    // this as a fresh attempt window (the counter is reset below before any
    // increment), so a single typo after expiry does not immediately re-lock.
    let mut failed_login_count = user.failed_login_count;
    if let Some(locked_until) = user.locked_until {
        if Utc::now() < locked_until {
            return Err(UserError::AccountLocked {
                until: locked_until,
            });
        }
        // Lock has expired — start a fresh attempt window.
        failed_login_count = 0;
    }

    if !password::verify(password_raw, &user.password_hash) {
        // Increment failure counter; lock if threshold reached.
        let new_count = failed_login_count + 1;
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
        "SELECT id, user_id, name, suspended_at, created_at, updated_at FROM merchants WHERE user_id = $1 LIMIT 1",
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

#[tracing::instrument(skip_all, err, fields(%user_id))]
pub async fn user_by_id(db: &PgPool, user_id: Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(&format!("SELECT {USER_COLS} FROM users WHERE id = $1"))
        .bind(user_id)
        .fetch_optional(db)
        .await
}

pub async fn user_profile_by_id(
    db: &PgPool,
    user_id: Uuid,
) -> Result<Option<UserProfile>, sqlx::Error> {
    sqlx::query_as::<_, UserProfile>(
        "SELECT id, email, name, is_admin, created_at
           FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

#[tracing::instrument(skip_all, err, fields(%user_id))]
pub async fn merchant_by_user(db: &PgPool, user_id: Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, suspended_at, created_at, updated_at FROM merchants WHERE user_id = $1 LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

#[tracing::instrument(skip_all, err, fields(%merchant_id))]
pub async fn merchant_by_id(
    db: &PgPool,
    merchant_id: Uuid,
) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, suspended_at, created_at, updated_at FROM merchants WHERE id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}

/// Erases a user's personal data (GDPR / NDPA right to erasure) while
/// keeping the rows that the financial records point at:
///
/// - email becomes `deleted-<user id>@deleted.invalid`, name and the
///   merchant name become "Deleted user", the phone number is cleared
/// - the password hash is blanked, so the accou

/* … truncated 4434 chars — edit only what you need near the top … */
