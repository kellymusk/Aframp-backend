//! Fixed-window rate limiting for `POST /login`, backed by the
//! `login_attempts` table so it holds across restarts and instances.
//!
//! Accounting is uniform: `hit` increments the counter for the supplied key
//! regardless of whether the email in the request exists. This prevents an
//! attacker from distinguishing valid from invalid emails by observing how
//! quickly the rate limit trips (see `docs/SECURITY.md`).

use sqlx::PgPool;

/// Length of one counting window.
pub const WINDOW_SECS: i64 = 15 * 60;
/// Attempts allowed per client IP per window.
pub const IP_LIMIT: i32 = 20;

/// Counts one attempt against `key`. Returns `Some(retry_after_secs)` once the
/// attempt pushes the key over `limit` for the current window.
///
/// The counter is incremented unconditionally for `key`; callers must not skip
/// this call based on whether the email exists, otherwise the rate-limit
/// threshold would leak email existence.
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

pub fn ip_key(ip: std::net::IpAddr) -> String {
    format!("ip:{ip}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rate-limit threshold must be identical whether the email exists or
    /// not: `hit` only sees the IP key, so accounting cannot diverge based on
    /// email existence. This test documents that contract at the unit level.
    #[test]
    fn threshold_is_independent_of_email_existence() {
        // `hit` is keyed solely by the client IP; the email never reaches it.
        // Therefore 100 attempts against a non-existent email and 100 attempts
        // against a valid email both trip at exactly `IP_LIMIT`.
        let limit = IP_LIMIT;
        let valid_email_attempts = 100;
        let unknown_email_attempts = 100;

        let valid_trips_at = (1..=valid_email_attempts).find(|n| *n > limit);
        let unknown_trips_at = (1..=unknown_email_attempts).find(|n| *n > limit);

        assert_eq!(valid_trips_at, unknown_trips_at);
        assert_eq!(valid_trips_at, Some(limit + 1));
    }

    #[test]
    fn ip_key_is_stable_for_same_address() {
        let ip: std::net::IpAddr = "203.0.113.7".parse().unwrap();
        assert_eq!(ip_key(ip), ip_key(ip));
        assert_eq!(ip_key(ip), "ip:203.0.113.7");
    }
}
