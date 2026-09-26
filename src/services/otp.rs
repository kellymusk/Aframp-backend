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
/// count against this — only brand-new challenges do.
const MAX_SENDS_PER_HOUR: i64 = 5;
const MAX_ATTEMPTS: i32 = 5;

/// OTP retention policy: audit records are kept for 24 hours after the
/// challenge's `expires_at` timestamp. This gives enough time for
/// debugging / support while preventing unbounded table growth.
///
/// At 5 challenges/hour per user across a large user base the table would
/// otherwise grow to tens of thousands of stale rows, making the
/// `WHERE consumed_at IS NULL AND expires_at > now()` queries progressively
/// slower. The `otp_challenges_expires_at_idx` index (migration 0009) makes
/// this DELETE efficient.
///
/// Call this periodically from a background task (e.g. every hour). It is
/// safe to call concurrently; Postgres row-level locking prevents double
/// deletes.
pub async fn cleanup_expired(db: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM otp_challenges WHERE expires_at < now() - interval '24 hours'",
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

#[derive(Debug, thiserror::Error)]
pub enum OtpError {
    #[error("too many requests, please try again shortly")]
    RateLimited,
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
    let email_taken: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE email = $1)")
        .bind(email)
        .fetch_one(db)
        .await?;
    if email_taken {
        return Err(OtpError::EmailTaken);
    }
    let phone_taken: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE phone_number = $1)")
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
    Ok(OtpChallengeResponse { challenge_id, expires_in_secs: CODE_TTL_SECS })
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
    Ok(OtpChallengeResponse { challenge_id, expires_in_secs: CODE_TTL_SECS })
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

    sqlx::query("UPDATE otp_challenges SET consumed_at = now() WHERE id = $1")
        .bind(challenge_id)
        .execute(db)
        .await?;

    if challenge.purpose == "login" {
        let user_id = challenge
            .user_id
            .expect("login challenge always carries user_id — enforced by the migration's CHECK constraint");
        let user = users::user_by_id(db, user_id)
            .await?
            .ok_or(OtpError::ChallengeNotFound)?;
        return Ok(VerifiedOutcome::Login(user));
    }

    // purpose == "signup": this is the only place a signup ever actually
    // creates the account — see the OTP plan for why gating just session
    // issuance (and not account creation) would be a bypass.
    let email = challenge
        .pending_email
        .as_deref()
        .expect("signup challenge always carries pending_email — enforced by the migration's CHECK constraint");
    let password_hash = challenge
        .pending_password_hash
        .as_deref()
        .expect("signup challenge always carries pending_password_hash");
    let name = challenge
        .pending_name
        .as_deref()
        .expect("signup challenge always carries pending_name");

    let (user, merchant) = users::create_verified(db, email, password_hash, name, &challenge.phone_number)
        .await
        .map_err(|err| match users::unique_violation_field(&err) {
            Some("users_email_key") => OtpError::EmailTaken,
            Some("users_phone_number_key") => OtpError::PhoneTaken,
            _ => OtpError::Database(err),
        })?;

    Ok(VerifiedOutcome::Signup(user, merchant))
}

struct NewChallenge<'a> {
    purpose: &'static str,
    user_id: Option<Uuid>,
    pending_email: Option<&'a str>,
    pending_password_hash: Option<&'a str>,
    pending_name: Option<&'a str>,
    phone_number: &'a str,
}

/// Refreshes a still-live challenge for this phone+purpose in place if one
/// exists (respecting the resend cooldown), otherwise inserts a fresh one
/// (respecting the hourly spam cap). Returns the challenge id and the plain
/// code — the only place the plain code exists outside an SMS.
async fn upsert_challenge(
    db: &PgPool,
    hmac_secret: &str,
    new: NewChallenge<'_>,
) -> Result<(Uuid, String), OtpError> {
    let existing: Option<(Uuid, chrono::DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, last_sent_at FROM otp_challenges
          WHERE phone_number = $1 AND purpose = $2 AND consumed_at IS NULL AND expires_at > now()
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(new.phone_number)
    .bind(new.purpose)
    .fetch_optional(db)
    .await?;

    if let Some((id, last_sent_at)) = existing {
        if Utc::now() - last_sent_at < Duration::seconds(RESEND_COOLDOWN_SECS) {
            return Err(OtpError::RateLimited);
        }
        let code = generate_code();
        let code_hash = hash_code(hmac_secret, id, &code);
        let expires_at = Utc::now() + Duration::seconds(CODE_TTL_SECS);
        sqlx::query(
            "UPDATE otp_challenges
                SET code_hash = $2, attempts = 0, expires_at = $3, last_sent_at = now(),
                    pending_email = $4, pending_password_hash = $5, pending_name = $6
              WHERE id = $1",
        )
        .bind(id)
        .bind(&code_hash)
        .bind(expires_at)
        .bind(new.pending_email)
        .bind(new.pending_password_hash)
        .bind(new.pending_name)
        .execute(db)
        .await?;
        return Ok((id, code));
    }

    let recent_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM otp_challenges
          WHERE phone_number = $1 AND purpose = $2 AND created_at > now() - interval '1 hour'",
    )
    .bind(new.phone_number)
    .bind(new.purpose)
    .fetch_one(db)
    .await?;
    if recent_count >= MAX_SENDS_PER_HOUR {
        return Err(OtpError::RateLimited);
    }

    let id = Uuid::new_v4();
    let code = generate_code();
    let code_hash = hash_code(hmac_secret, id, &code);
    let expires_at = Utc::now() + Duration::seconds(CODE_TTL_SECS);
    sqlx::query(
        "INSERT INTO otp_challenges
             (id, purpose, user_id, pending_email, pending_password_hash, pending_name, phone_number, code_hash, max_attempts, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(new.purpose)
    .bind(new.user_id)
    .bind(new.pending_email)
    .bind(new.pending_password_hash)
    .bind(new.pending_name)
    .bind(new.phone_number)
    .bind(&code_hash)
    .bind(MAX_ATTEMPTS)
    .bind(expires_at)
    .execute(db)
    .await?;

    Ok((id, code))
}

async fn send_code(otp: &dyn OtpProvider, phone: &str, code: &str) -> Result<(), OtpError> {
    let message = format!("Your Aframp verification code is {code}. It expires in 10 minutes.");
    otp.send_sms(phone, &message).await.map_err(OtpError::SendFailed)
}

fn generate_code() -> String {
    let mut bytes = [0u8; 4];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let n = u32::from_le_bytes(bytes) % 1_000_000;
    format!("{n:06}")
}

/// A 6-digit code has only 1,000,000 possible values, so a bare hash of it —
/// even salted — is trivially reversible by anyone with DB read access
/// (precompute all million hashes once, look up forever; the search space
/// is the bottleneck, not precomputation). HMAC keyed with a secret that
/// lives outside the database is what actually stops that. The challenge id
/// is folded into the MAC input too, cheap defense-in-depth given lookups
/// are already scoped `WHERE id = $challenge_id`.
fn hash_code(hmac_secret: &str, challenge_id: Uuid, code: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(hmac_secret.as_bytes())
        .expect("HMAC-SHA256 accepts a key of any length");
    mac.update(challenge_id.as_bytes());
    mac.update(code.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn verify_code(hmac_secret: &str, challenge_id: Uuid, code: &str, stored_hash_hex: &str) -> bool {
    let Ok(stored_bytes) = hex::decode(stored_hash_hex) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(hmac_secret.as_bytes()) else {
        return false;
    };
    mac.update(challenge_id.as_bytes());
    mac.update(code.as_bytes());
    mac.verify_slice(&stored_bytes).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_is_deterministic_and_code_sensitive() {
        let id = Uuid::new_v4();
        let a = hash_code("secret", id, "123456");
        let b = hash_code("secret", id, "123456");
        let c = hash_code("secret", id, "654321");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn verify_rejects_tampered_digest() {
        let id = Uuid::new_v4();
        let hash = hash_code("secret", id, "123456");
        assert!(verify_code("secret", id, "123456", &hash));
        assert!(!verify_code("secret", id, "000000", &hash));
        assert!(!verify_code("different-secret", id, "123456", &hash));
    }
}
