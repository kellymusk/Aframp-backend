//! Fixed-window rate limiting for `POST /login`, backed by the
//! `login_attempts` table so it holds across restarts and instances.

use sqlx::PgPool;

/// Length of one counting window.
pub const WINDOW_SECS: i64 = 15 * 60;
/// Attempts allowed per email address per window.
pub const EMAIL_LIMIT: i32 = 5;
/// Attempts allowed per client IP per window.
pub const IP_LIMIT: i32 = 20;

/// Counts one attempt against `key`. Returns `Some(retry_after_secs)` once the
/// attempt pushes the key over `limit` for the current window.
pub async fn hit(db: &PgPool, key: &str, limit: i32) -> Result<Option<i64>, sqlx::Error> {
    let (attempts, retry_after): (i32, i64) = sqlx::query_as(
        "INSERT INTO login_attempts (key, window_start, attempts)
         VALUES ($1, now(), 1)
         ON CONFLICT (key) DO UPDATE SET
           attempts = CASE
             WHEN login_attempts.window_start <= now() - $2 * interval '1 second' THEN 1
             ELSE login_attempts.attempts + 1
           END,
           window_start = CASE
             WHEN login_attempts.window_start <= now() - $2 * interval '1 second' THEN now()
             ELSE login_attempts.window_start
           END
         RETURNING attempts,
                   GREATEST(1, CEIL(EXTRACT(EPOCH FROM
                     window_start + $2 * interval '1 second' - now())))::BIGINT",
    )
    .bind(key)
    .bind(WINDOW_SECS as f64)
    .fetch_one(db)
    .await?;
    Ok((attempts > limit).then_some(retry_after))
}

/// Clears the counter for `key` (after a successful login) and drops any
/// counters whose window has long elapsed.
pub async fn reset(db: &PgPool, key: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM login_attempts
          WHERE key = $1 OR window_start < now() - $2 * interval '1 second'",
    )
    .bind(key)
    .bind(WINDOW_SECS as f64)
    .execute(db)
    .await
    .map(|_| ())
}

pub fn email_key(email: &str) -> String {
    format!("email:{}", email.trim().to_lowercase())
}

pub fn ip_key(ip: std::net::IpAddr) -> String {
    format!("ip:{ip}")
}
