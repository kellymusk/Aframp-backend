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

/// Number of decimal digits in a generated OTP code.
///
/// Why 6 digits: it is the de-facto UX standard for SMS OTPs (users expect to
/// type six digits, and SMS gateways/autofill handle it well). The keyspace is
/// `10^6 = 1_000_000` values, which is only safe because of the layered
/// mitigations below — the code length alone is *not* the security boundary.
const CODE_DIGITS: u32 = 6;
/// Size of the OTP keyspace (`10^CODE_DIGITS`).
const CODE_KEYSpace: u64 = 1_000_000;

/// Acceptable per-challenge brute-force success probability.
///
/// With `MAX_ATTEMPTS = 5` guesses against a `1_000_000`-value keyspace, an
/// attacker who knows the public `challenge_id` succeeds with probability
/// `5 / 1_000_000 = 0.0005%` per challenge. That is the accepted risk
/// threshold: below 0.001% per challenge. It is only acceptable because the
/// window is bounded by `CODE_TTL_SECS` (10 minutes), the attempt budget is
/// capped at `MAX_ATTEMPTS` (5), and login issuance is additionally gated by
/// per-IP rate limiting. If any of those mitigations is removed, the digit
/// count must be revisited (e.g. 8 digits → `5 / 100_000_000`).
const ACCEPTABLE_PER_CHALLENGE_SUCCESS_PCT: f64 = 0.001;

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

/// Normalises an email address for the signup uniqueness check.
///
/// Product decision (issue #1423): `is_valid_email` accepts RFC 5322
/// `+`-extension addresses (`user+tag@example.com`), but the local part is
/// normalised before the uniqueness check so that `alice@example.com` and
/// `alice+bypass@example.com` are treated as the same inbox. Without this a
/// banned/suspended merchant could re-signup with `+1`, `+2`, … and hold
/// multiple accounts on the same inbox.
///
/// The stored email is left untouched (the user keeps the address they typed);
/// only the comparison key is normalised. The domain is lowercased and the
/// `+tag` suffix is stripped from the local part. This is a known caveat for
/// providers that treat `+` as a significant character, which is rare.
pub fn normalise_email_for_uniqueness(email: &str) -> String {
    let email = email.trim();
    match email.rsplit_once('@') {
        Some((local, domain)) => {
            let local = local.split('+').next().unwrap_or(local);
            format!("{}@{}", local, domain.to_lowercase())
        }
        None => email.to_lowercase(),
    }
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
    // Compare on the normalised local part so `+`-extension aliases of an
    // already-registered inbox are rejected (issue #1423).
    let normalised_email = normalise_email_for_uniqueness(email);
    let email_taken: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE lower(split_part(email, '+', 1)) = $1)",
    )
    .bind(&normalised_email)
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

    if challenge.purpose == "login" 

/* … truncated 8988 chars — edit only what you need near the top … */
