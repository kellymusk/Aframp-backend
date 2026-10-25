use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::password;
use crate::models::{Merchant, User, UserProfile};
use crate::models::merchant::Merchant;
use crate::models::user::User;

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

/// A user row joined with their (optional) merchant row, fetched in a single
/// roundtrip via LEFT JOIN. Merchant columns are `None` when the user has no
/// merchant.
#[derive(Debug)]
pub struct UserWithMerchant {
    pub user: User,
    pub merchant: Option<Merchant>,
}

/// Materializes a brand-new account. The only caller is
/// `services::otp::verify`, once a signup's OTP challenge passes — never
/// called speculatively, so an unverified phone can never end up attached
/// to a real, loggable-in account. `password_hash` must already be hashed.
///
/// Data model: a merchant's `name` mirrors its owning user's `name` — there
/// is no separate business-name field. Both rows are written here with the
/// same `name`, and any later profile rename must update `users.name` and
/// `merchants.name` together in one transaction (see `update_name`) so the
/// admin view and merchant profile never disagree.
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

/// Renames a user and keeps the linked merchant row in sync. Both writes
/// happen in a single transaction so `users.name` and `merchants.name` can
/// never diverge (see the data-model note on `create_verified`). Returns the
/// updated user and, if one exists, the updated merchant.
pub async fn update_name(
    db: &PgPool,
    user_id: uuid::Uuid,
    name: &str,
) -> Result<(User, Option<Merchant>), sqlx::Error> {
    let mut tx = db.begin().await?;
    let user = sqlx::query_as::<_, User>(
        "UPDATE users SET name = $2, updated_at = now()
          WHERE id = $1
          RETURNING id, email, password_hash, name, is_admin, phone_number, phone_verified, created_at, updated_at",
    )
    .bind(user_id)
    .bind(name)
    .fetch_one(&mut *tx)
    .await?;

    let merchant = sqlx::query_as::<_, Merchant>(
        "UPDATE merchants SET name = $2
          WHERE user_id = $1
          RETURNING id, user_id, name, created_at",
    )
    .bind(user_id)
    .bind(name)
    .fetch_optional(&mut *tx)
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

/// Fetches a user and their optional merchant in a single LEFT JOIN query.
pub async fn user_with_merchant_by_email(
    db: &PgPool,
    email: &str,
) -> Result<Option<UserWithMerchant>, sqlx::Error> {
    let row = sqlx::query_as::<_, (User, Option<Merchant>)>(
        "SELECT u.id, u.email, u.password_hash, u.name, u.is_admin, u.phone_number, u.phone_verified, u.created_at, u.updated_at,
                m.id, m.user_id, m.name, m.created_at
           FROM users u
           LEFT JOIN merchants m ON m.user_id = u.id
          WHERE u.email = $1
          LIMIT 1",
pub async fn login(
    db: &PgPool,
    email: &str,
    password_raw: &str,
) -> Result<(User, Option<Merchant>), UserError> {
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
    .await?;

    Ok(row.map(|(user, merchant)| UserWithMerchant { user, merchant }))
}

pub async fn login(db: &PgPool, email: &str, password_raw: &str) -> Result<(User, Option<Merchant>), UserError> {
    let UserWithMerchant { user, merchant } = user_with_merchant_by_email(db, email)
        .await?
        .ok_or(UserError::InvalidCredentials)?;
    // Equalize response time between the "unknown email" and "wrong password"
    // paths: when no user is found we still run a full Argon2 verification
    // against a dummy hash (same parameters as real hashes) so the timing of
    // the two failure modes is indistinguishable. This closes the username
    // enumeration timing side-channel.
    let user = match user {
        Some(user) => user,
        None => {
            password::verify(password_raw, password::DUMMY_HASH);
            return Err(UserError::InvalidCredentials);
        }
    };

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
pub async fn user_by_id(db: &PgPool, user_id: Uuid) -> sqlx::Result<Option<User>> {
    sqlx::query_as::<_, User>(
        "SELECT id, email, name, is_admin, created_at FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn user_profile_by_id(
    db: &PgPool,
    user_id: uuid::Uuid,
) -> Result<Option<UserProfile>, sqlx::Error> {
    sqlx::query_as::<_, UserProfile>(
        "SELECT id, email, name, is_admin, created_at
           FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn merchant_by_user(
    db: &PgPool,
    user_id: uuid::Uuid,
) -> Result<Option<Merchant>, sqlx::Error> {
pub async fn merchant_by_user(db: &PgPool, user_id: Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, suspended_at, created_at FROM merchants WHERE user_id = $1 LIMIT 1",
pub async fn merchant_by_user(db: &PgPool, user_id: Uuid) -> sqlx::Result<Option<Merchant>> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at, updated_at FROM merchants WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn merchant_by_id(
    db: &PgPool,
    merchant_id: uuid::Uuid,
) -> Result<Option<Merchant>, sqlx::Error> {
pub async fn merchant_by_id(db: &PgPool, merchant_id: Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, suspended_at, created_at FROM merchants WHERE id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the LEFT JOIN yields `merchant: None` for a user with no
    /// merchant row. Requires a live `DATABASE_URL`; skipped otherwise.
    #[tokio::test]
    async fn user_with_merchant_by_email_no_merchant() {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            return;
    /// Verifies that renaming a user keeps `users.name` and `merchants.name`
    /// in sync after `update_name`. Requires a live `DATABASE_URL`; skipped
    /// otherwise so the suite still runs without a database.
    #[tokio::test]
    async fn update_name_keeps_merchant_in_sync() {
        let url = match std::env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => return,
        };
        let db = match PgPool::connect(&url).await {
            Ok(db) => db,
            Err(_) => return,
        };

        let email = format!("no-merchant-{}@example.test", uuid::Uuid::new_v4());
        let user = sqlx::query_as::<_, User>(
            "INSERT INTO users (email, password_hash, name, phone_number, phone_verified)
             VALUES ($1, $2, $3, $4, true)
             RETURNING id, email, password_hash, name, is_admin, phone_number, phone_verified, created_at, updated_at",
        )
        .bind(&email)
        .bind("x")
        .bind("No Merchant")
        .bind("+10000000000")
        .fetch_one(&db)
        .await
        .expect("insert user");

        let result = user_with_merchant_by_email(&db, &email)
            .await
            .expect("query")
            .expect("user present");

        assert_eq!(result.user.id, user.id);
        assert!(result.merchant.is_none());

        let email = format!("name-sync-{}@example.test", uuid::Uuid::new_v4());
        let phone = format!("+234{}", uuid::Uuid::new_v4().as_u128() % 1_000_000_0000);
        let (user, merchant) = create_verified(&db, &email, "hash", "Old Name", &phone)
            .await
            .expect("create_verified");
        assert_eq!(merchant.name, "Old Name");

        let (updated_user, updated_merchant) = update_name(&db, user.id, "New Name")
            .await
            .expect("update_name");
        assert_eq!(updated_user.name, "New Name");
        let updated_merchant = updated_merchant.expect("merchant should exist");
        assert_eq!(updated_merchant.name, "New Name");

        let fetched = merchant_by_user(&db, user.id)
            .await
            .expect("merchant_by_user")
            .expect("merchant row");
        assert_eq!(fetched.name, updated_user.name);

        // Cleanup so repeated runs don't collide.
        let _ = sqlx::query("DELETE FROM merchants WHERE user_id = $1")
            .bind(user.id)
            .execute(&db)
            .await;
        let _ = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user.id)
            .execute(&db)
            .await;
    }
/// Erases a user's personal data (GDPR / NDPA right to erasure) while
/// keeping the rows that the financial records point at:
///
/// - email becomes `deleted-<user id>@deleted.invalid`, name and the
///   merchant name become "Deleted user", the phone number is cleared
/// - the password hash is blanked, so the account can never log in again
/// - pending OTP challenges (which can hold PII) are removed
/// - `deleted_at` is set, which makes every outstanding session token for
///   this user fail authentication
///
/// Payments, payment requests, withdrawals, wallets and balances are left
/// intact. Returns `false` if the user doesn't exist or was already deleted.
pub async fn anonymize_and_delete(db: &PgPool, user_id: uuid::Uuid) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    let updated = sqlx::query(
        "UPDATE users
            SET email = 'deleted-' || id::text || '@deleted.invalid',
                name = 'Deleted user',
                phone_number = NULL,
                phone_verified = false,
                password_hash = '',
                deleted_at = now(),
                updated_at = now()
          WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Ok(false);
    }
    sqlx::query("UPDATE merchants SET name = 'Deleted user' WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM otp_challenges WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// Whether the account exists and hasn't been deleted.
pub async fn is_active(db: &PgPool, user_id: uuid::Uuid) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, bool>("SELECT deleted_at IS NULL FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await?
        .unwrap_or(false))
/// Whether a session that began at `session_start` (unix seconds — the
/// token's `orig_iat`) is still valid: the account must be active and the
/// session must not predate the last password change.
pub async fn session_is_valid(
    db: &PgPool,
    user_id: Uuid,
    session_start: i64,
) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT deleted_at IS NULL
                AND (password_changed_at IS NULL
                     OR $2 >= floor(extract(epoch FROM password_changed_at))::bigint)
           FROM users WHERE id = $1",
    )
    .bind(user_id)
    .bind(session_start)
    .fetch_optional(db)
    .await?
    .unwrap_or(false))
}

#[derive(Debug, thiserror::Error)]
pub enum ChangePasswordError {
    #[error("current password is incorrect")]
    WrongCurrentPassword,
    #[error("failed to hash password")]
    Hash,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Verifies `current_password` against the stored hash, then stores the hash
/// of `new_password` and stamps `password_changed_at`, which invalidates every
/// session issued before now (see [`session_is_valid`]). Returns the change
/// time so the caller can issue a replacement session that survives it.
pub async fn change_password(
    db: &PgPool,
    user_id: Uuid,
    current_password: &str,
    new_password: &str,
) -> Result<chrono::DateTime<Utc>, ChangePasswordError> {
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT password_hash FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    let Some(stored) = stored else {
        return Err(ChangePasswordError::WrongCurrentPassword);
    };
    if !password::verify(current_password, &stored) {
        return Err(ChangePasswordError::WrongCurrentPassword);
    }
    let new_hash = password::hash(new_password).map_err(|_| ChangePasswordError::Hash)?;
    // Stamped from the app clock, not Postgres `now()`: it's compared against
    // JWT `iat`s minted by this process, so both sides must share a clock.
    let changed_at = Utc::now();
    sqlx::query(
        "UPDATE users
            SET password_hash = $2, password_changed_at = $3, updated_at = now()
          WHERE id = $1",
    )
    .bind(user_id)
    .bind(&new_hash)
    .bind(changed_at)
    .execute(db)
    .await?;
    Ok(changed_at)
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
