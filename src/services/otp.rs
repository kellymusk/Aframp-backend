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
/// Number of retries for transient SMS send failures (network blips, 5xx).
const SEND_MAX_RETRIES: u32 = 2;
/// Delay between retries of a transient SMS send failure.
const SEND_RETRY_DELAY_MS: u64 = 500;

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

/// Sends the OTP SMS, retrying transient failures (network blips, provider
/// 5xx) up to `SEND_MAX_RETRIES` times with a short delay. Permanent failures
/// (e.g. 400 invalid phone) are returned immediately without retrying.
async fn send_code(otp: &dyn OtpProvider, phone: &str, code: &str) -> Result<(), OtpError> {
    let message = format!("Your Aframp verification code is {code}. It expires in 10 minutes.");

    let mut attempt = 0;
    loop {
        match otp.send_sms(phone, &message).await {
            Ok(()) => return Ok(()),
            Err(err) if err.is_transient() && attempt < SEND_MAX_RETRIES => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(SEND_RETRY_DELAY_MS)).await;
            }
            Err(err) => return Err(OtpError::SendFailed(err.to_string())),
        }
    }
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

    let recent_sends: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM otp_challenges
          WHERE phone_number = $1 AND purpose = $2 AND created_at > now() - interval '1 hour'",
    )
    .bind(new.phone_number)
    .bind(new.purpose)
    .fetch_one(db)
    .await?;
    if recent_sends >= MAX_SENDS_PER_HOUR {
        return Err(OtpError::RateLimited);
    }

    let id = Uuid::new_v4();
    let code = generate_code();
    let code_hash = hash_code(hmac_secret, id, &code);
    let expires_at = Utc::now() + Duration::seconds(CODE_TTL_SECS);
    sqlx::query(
        "INSERT INTO otp_challenges
            (id, purpose, user_id, pending_email, pending_password_hash, pending_name,
             phone_number, code_hash, attempts, max_attempts, expires_at, last_sent_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 0, $9, $10, now())",
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

fn generate_code() -> String {
    let mut bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut bytes);
    let n = u32::from_be_bytes(bytes) % 1_000_000;
    format!("{n:06}")
}

fn hash_code(secret: &str, challenge_id: Uuid, code: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts keys of any length");
    mac.update(challenge_id.as_bytes());
    mac.update(code.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn verify_code(secret: &str, challenge_id: Uuid, code: &str, expected_hash: &str) -> bool {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts keys of any length");
    mac.update(challenge_id.as_bytes());
    mac.update(code.as_bytes());
    let computed = hex::encode(mac.finalize().into_bytes());
    // Constant-time comparison to avoid leaking the hash via timing.
    computed.len() == expected_hash.len()
        && computed
            .bytes()
            .zip(expected_hash.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::otp::OtpSendError;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Provider that fails transiently on the first call, then succeeds.
    struct FlakyProvider {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl OtpProvider for FlakyProvider {
        async fn send_sms(&self, _phone: &str, _message: &str) -> Result<(), OtpSendError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err(OtpSendError::Transient("network timeout".to_string()))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn send_code_retries_transient_failure_and_succeeds() {
        let provider = FlakyProvider { calls: AtomicUsize::new(0) };
        let result = send_code(&provider, "+15555550100", "123456").await;
        assert!(result.is_ok(), "expected retry to succeed, got {result:?}");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    }
}
