use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Merchant, OtpChallenge, OtpChallengeResponse, User};
use crate::otp::OtpProvider;
use crate::services::users;

/// How long a code is valid for.
const CODE_TTL_SECS: i64 = 600;
/// Minimum gap between two sends to the same phone for the same purpose —
/// re-POSTing signup/login *is* the resend path, so this is what stops it
/// from being spammed.
const RESEND_COOLDOWN_SECS: i64 = 60;
/// Spam cap: at most this many fresh challenges per phone+purpose per hour.
/// A refreshed (cooldown-respecting) resend of an existing challenge doesn't
/// count against this — only brand-new challenges do. Verified (consumed)
/// challenges are excluded, so someone who completes each code isn't locked
/// out by their own past successes.
const MAX_SENDS_PER_HOUR: i64 = 5;
const MAX_ATTEMPTS: i32 = 5;

#[derive(Debug, thiserror::Error)]
pub enum OtpError {
    #[error("too many requests, please try again shortly")]
    RateLimited {
        /// Seconds until a new code may be requested.
        retry_after_secs: u64,
    },
    #[error("otp challenge not found or already used")]
    ChallengeNotFound,
    #[error("otp code has expired")]
    Expired,
    #[error("too many incorrect attempts")]
    Locked,
    #[error("incorrect code")]
    InvalidCode,
    #[error("email already registered")]
    EmailTaken,
    #[error("phone number already registered")]
    PhoneTaken,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("failed to send SMS: {0}")]
    SendFailed(String),
}

pub enum VerifiedOutcome {
    Login(User),
    Signup(User, Merchant),
    /// A `phone_change` challenge was verified; the user's phone number has
    /// been switched to the newly verified one.
    PhoneChanged(User),
}

pub async fn start_signup_challenge(
    db: &PgPool,
    otp: &dyn OtpProvider,
    hmac_secret: &str,
    email: &str,
    password_hash: &str,
    name: &str,
    phone_number: &str,
) -> Result<OtpChallengeResponse, OtpError> {
    let email_taken: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE email = $1)")
            .bind(email)
            .fetch_one(db)
            .await?;
    if email_taken {
        return Err(OtpError::EmailTaken);
    }
    let phone_taken: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE phone_number = $1)")
            .bind(phone_number)
            .fetch_one(db)
            .await?;
    if phone_taken {
        return Err(OtpError::PhoneTaken);
    }

    let (challenge_id, code) = upsert_challenge(
        db,
        hmac_secret,
        NewChallenge {
            purpose: "signup",
            user_id: None,
            pending_email: Some(email),
            pending_password_hash: Some(password_hash),
            pending_name: Some(name),
            phone_number,
        },
    )
    .await?;

    send_code(otp, phone_number, &code).await?;
    Ok(OtpChallengeResponse {
        challenge_id,
        expires_in_secs: CODE_TTL_SECS,
    })
}

/// Sends a code to `new_phone` to prove the user controls it before
/// `PATCH /me` switches their number (the same OTP flow as signup). The
/// number isn't changed until the challenge is verified via `/verify-otp`.
pub async fn start_phone_change_challenge(
    db: &PgPool,
    otp: &dyn OtpProvider,
    hmac_secret: &str,
    user_id: Uuid,
    new_phone: &str,
) -> Result<OtpChallengeResponse, OtpError> {
    let phone_taken: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE phone_number = $1 AND id <> $2)",
    )
    .bind(new_phone)
    .bind(user_id)
    .fetch_one(db)
    .await?;
    if phone_taken {
        return Err(OtpError::PhoneTaken);
    }

    let (challenge_id, code) = upsert_challenge(
        db,
        hmac_secret,
        NewChallenge {
            purpose: "phone_change",
            user_id: Some(user_id),
            pending_email: None,
            pending_password_hash: None,
            pending_name: None,
            phone_number: new_phone,
        },
    )
    .await?;

    send_code(otp, new_phone, &code).await?;
    Ok(OtpChallengeResponse {
        challenge_id,
        expires_in_secs: CODE_TTL_SECS,
    })
}

pub async fn start_login_challenge(
    db: &PgPool,
    otp: &dyn OtpProvider,
    hmac_secret: &str,
    user: &User,
) -> Result<OtpChallengeResponse, OtpError> {
    let phone_number = user
        .phone_number
        .as_deref()
        .expect("caller must only call this when the account has a phone on file");

    let (challenge_id, code) = upsert_challenge(
        db,
        hmac_secret,
        NewChallenge {
            purpose: "login",
            user_id: Some(user.id),
            pending_email: None,
            pending_password_hash: None,
            pending_name: None,
            phone_number,
        },
    )
    .await?;

    send_code(otp, phone_number, &code).await?;
    Ok(OtpChallengeResponse {
        challenge_id,
        expires_in_secs: CODE_TTL_SECS,
    })
}

pub async fn verify(
    db: &PgPool,
    hmac_secret: &str,
    challenge_id: Uuid,
    code: &str,
) -> Result<VerifiedOutcome, OtpError> {
    let challenge = sqlx::query_as::<_, OtpChallenge>(
        "SELECT id, purpose, user_id, pending_email, pending_password_hash, pending_name,
                phone_number, code_hash, attempts, max_attempts, expires_at, consumed_at,
                last_sent_at, created_at
           FROM otp_challenges WHERE id = $1",
    )
    .bind(challenge_id)
    .fetch_optional(db)
    .await?
    .filter(|c| c.consumed_at.is_none())
    .ok_or(OtpError::ChallengeNotFound)?;

    if Utc::now() > challenge.expires_at {
        return Err(OtpError::Expired);
    }
    if challenge.attempts >= challenge.max_attempts {
        return Err(OtpError::Locked);
    }

    if !verify_code(hmac_secret, challenge_id, code, &challenge.code_hash) {
        sqlx::query("UPDATE otp_challenges SET attempts = attempts + 1 WHERE id = $1")
            .bind(challenge_id)
            .execute(db)
            .await?;
        return Err(OtpError::InvalidCode);
    }

    // Delete the challenge as soon as the code checks out: a signup row holds
    // the pending password hash, which must not outlive its purpose. The
    // DELETE is also the atomic claim — if a concurrent verify already took
    // this challenge, nothing is deleted and this attempt fails.
    let claimed = sqlx::query("DELETE FROM otp_challenges WHERE id = $1")
        .bind(challenge_id)
        .execute(db)
        .await?
        .rows_affected();
    if claimed == 0 {
        return Err(OtpError::ChallengeNotFound);
    }

    if challenge.purpose == "login" {
        let user_id = challenge.user_id.expect(
            "login challenge always carries user_id — enforced by the migration's CHECK constraint",
        );
        let user = users::user_by_id(db, user_id)
            .await?
            .ok_or(OtpError::ChallengeNotFound)?;
        return Ok(VerifiedOutcome::Login(user));
    }

    if challenge.purpose == "phone_change" {
        let user_id = challenge
            .user_id
            .expect("phone_change challenge always carries user_id — enforced by the migration's CHECK constraint");
        let user = sqlx::query_as::<_, User>(&format!(
            "UPDATE users SET phone_number = $2, phone_verified = true, updated_at = now()
              WHERE id = $1
              RETURNING {}",
            users::USER_COLS
        ))
        .bind(user_id)
        .bind(&challenge.phone_number)
        .fetch_optional(db)
        .await
        .map_err(|err| match users::unique_violation_field(&err) {
            Some("users_phone_number_key") => OtpError::PhoneTaken,
            _ => OtpError::Database(err),
        })?
        .ok_or(OtpError::ChallengeNotFound)?;
        return Ok(VerifiedOutcome::PhoneChanged(user));
    }

    // Signup: create the user + merchant now that the phone is proven.
    let email = challenge
        .pending_email
        .as_deref()
        .expect("signup challenge always carries pending_email");
    let password_hash = challenge
        .pending_password_hash
        .as_deref()
        .expect("signup challenge always carries pending_password_hash");
    let name = challenge
        .pending_name
        .as_deref()
        .expect("signup challenge always carries pending_name");

    let (user, merchant) = users::create_user_with_merchant(
        db,
        email,
        password_hash,
        name,
        &challenge.phone_number,
    )
    .await
    .map_err(|err| match users::unique_violation_field(&err) {
        Some("users_email_key") => OtpError::EmailTaken,
        Some("users_phone_number_key") => OtpError::PhoneTaken,
        _ => OtpError::Database(err),
    })?;

    Ok(VerifiedOutcome::Signup(user, merchant))
}

/// Deletes challenges that are both terminal (consumed or expired) and older
/// than 24 hours. This is the only mechanism preventing accumulated PII
/// (pending password hashes) from persisting indefinitely.
pub async fn purge_stale_challenges(db: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM otp_challenges
          WHERE (consumed_at IS NOT NULL OR expires_at < now())
            AND created_at < now() - interval '24 hours'",
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

struct NewChallenge<'a> {
    purpose: &'a str,
    user_id: Option<Uuid>,
    pending_email: Option<&'a str>,
    pending_password_hash: Option<&'a str>,
    pending_name: Option<&'a str>,
    phone_number: &'a str,
}

async fn upsert_challenge(
    db: &PgPool,
    hmac_secret: &str,
    new: NewChallenge<'_>,
) -> Result<(Uuid, String), OtpError> {
    let now = Utc::now();

    // Reuse an existing, still-valid challenge for this phone+purpose instead
    // of creating a new row, so a resend doesn't multiply rows.
    let existing = sqlx::query_as::<_, OtpChallenge>(
        "SELECT id, purpose, user_id, pending_email, pending_password_hash, pending_name,
                phone_number, code_hash, attempts, max_attempts, expires_at, consumed_at,
                last_sent_at, created_at
           FROM otp_challenges
          WHERE phone_number = $1 AND purpose = $2 AND consumed_at IS NULL
            AND expires_at > now()
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(new.phone_number)
    .bind(new.purpose)
    .fetch_optional(db)
    .await?;

    if let Some(existing) = existing {
        let elapsed = now.signed_duration_since(existing.last_sent_at);
        if elapsed < Duration::seconds(RESEND_COOLDOWN_SECS) {
            let retry_after_secs = (RESEND_COOLDOWN_SECS - elapsed.num_seconds()).max(0) as u64;
            return Err(OtpError::RateLimited { retry_after_secs });
        }

        let code = generate_code();
        let code_hash = hash_code(hmac_secret, existing.id, &code);
        sqlx::query(
            "UPDATE otp_challenges
                SET code_hash = $2, attempts = 0, expires_at = $3, last_sent_at = now()
              WHERE id = $1",
        )
        .bind(existing.id)
        .bind(&code_hash)
        .bind(now + Duration::seconds(CODE_TTL_SECS))
        .execute(db)
        .await?;
        return Ok((existing.id, code));
    }

    // Spam cap: count fresh challenges created in the last hour for this
    // phone+purpose. Consumed rows are excluded so completed flows don't
    // count against the user.
    let recent_sends: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM otp_challenges
          WHERE phone_number = $1 AND purpose = $2 AND consumed_at IS NULL
            AND created_at > now() - interval '1 hour'",
    )
    .bind(new.phone_number)
    .bind(new.purpose)
    .fetch_one(db)
    .await?;
    if recent_sends >= MAX_SENDS_PER_HOUR {
        return Err(OtpError::RateLimited {
            retry_after_secs: 3600,
        });
    }

    let challenge_id = Uuid::new_v4();
    let code = generate_code();
    let code_hash = hash_code(hmac_secret, challenge_id, &code);

    sqlx::query(
        "INSERT INTO otp_challenges
            (id, purpose, user_id, pending_email, pending_password_hash, pending_name,
             phone_number, code_hash, attempts, max_attempts, expires_at, last_sent_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 0, $9, $10, now())",
    )
    .bind(challenge_id)
    .bind(new.purpose)
    .bind(new.user_id)
    .bind(new.pending_email)
    .bind(new.pending_password_hash)
    .bind(new.pending_name)
    .bind(new.phone_number)
    .bind(&code_hash)
    .bind(MAX_ATTEMPTS)
    .bind(now + Duration::seconds(CODE_TTL_SECS))
    .execute(db)
    .await?;

    Ok((challenge_id, code))
}

async fn send_code(otp: &dyn OtpProvider, phone_number: &str, code: &str) -> Result<(), OtpError> {
    otp.send(phone_number, code)
        .await
        .map_err(|err| OtpError::SendFailed(err.to_string()))
}

fn generate_code() -> String {
    let mut bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut bytes);
    let n = u32::from_be_bytes(bytes) % 1_000_000;
    format!("{n:06}")
}

fn hash_code(hmac_secret: &str, challenge_id: Uuid, code: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(hmac_secret.as_bytes())
        .expect("HMAC accepts keys of any length");
    mac.update(challenge_id.as_bytes());
    mac.update(code.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn verify_code(hmac_secret: &str, challenge_id: Uuid, code: &str, expected: &str) -> bool {
    let actual = hash_code(hmac_secret, challenge_id, code);
    // Constant-time comparison to avoid leaking the hash via timing.
    actual.len() == expected.len()
        && actual
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    /// Connects to the test database from `DATABASE_URL`. Returns `None` when
    /// the variable is unset so the suite is skipped in environments without
    /// a database (e.g. plain `cargo test` on a laptop).
    async fn test_pool() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .ok()
    }

    /// Inserts a challenge row with explicit `created_at`, `expires_at` and
    /// `consumed_at` so the purge predicate can be exercised directly.
    async fn insert_challenge(
        db: &PgPool,
        phone_number: &str,
        created_at: chrono::DateTime<Utc>,
        expires_at: chrono::DateTime<Utc>,
        consumed_at: Option<chrono::DateTime<Utc>>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO otp_challenges
                (id, purpose, phone_number, code_hash, attempts, max_attempts,
                 expires_at, consumed_at, last_sent_at, created_at)
             VALUES ($1, 'login', $2, 'test-hash', 0, 5, $3, $4, $5, $5)",
        )
        .bind(id)
        .bind(phone_number)
        .bind(expires_at)
        .bind(consumed_at)
        .bind(created_at)
        .execute(db)
        .await
        .expect("insert test challenge");
        id
    }

    async fn row_exists(db: &PgPool, id: Uuid) -> bool {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM otp_challenges WHERE id = $1)")
            .bind(id)
            .fetch_one(db)
            .await
            .expect("check row existence")
    }

    #[tokio::test]
    async fn purge_removes_consumed_challenge_older_than_24h() {
        let Some(db) = test_pool().await else {
            return;
        };
        let now = Utc::now();
        let id = insert_challenge(
            &db,
            "+15550000001",
            now - Duration::hours(25),
            now - Duration::hours(24),
            Some(now - Duration::hours(24)),
        )
        .await;

        purge_stale_challenges(&db).await.expect("purge");

        assert!(!row_exists(&db, id).await, "consumed stale row should be purged");
    }

    #[tokio::test]
    async fn purge_keeps_unconsumed_unexpired_challenge_older_than_24h() {
        let Some(db) = test_pool().await else {
            return;
        };
        let now = Utc::now();
        let id = insert_challenge(
            &db,
            "+15550000002",
            now - Duration::hours(25),
            now + Duration::hours(1),
            None,
        )
        .await;

        purge_stale_challenges(&db).await.expect("purge");

        assert!(
            row_exists(&db, id).await,
            "unconsumed, non-expired row must not be purged"
        );
    }

    #[tokio::test]
    async fn purge_removes_challenge_expired_older_than_24h() {
        let Some(db) = test_pool().await else {
            return;
        };
        let now = Utc::now();
        let id = insert_challenge(
            &db,
            "+15550000003",
            now - Duration::hours(26),
            now - Duration::hours(25),
            None,
        )
        .await;

        purge_stale_challenges(&db).await.expect("purge");

        assert!(!row_exists(&db, id).await, "expired stale row should be purged");
    }
}
