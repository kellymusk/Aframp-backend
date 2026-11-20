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

    // Signup: create the user + merchant from the pending fields.
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

    // Reuse an existing, still-valid challenge for the same phone+purpose
    // instead of minting a new one, so a resend doesn't reset the attempt
    // counter (which would let an attacker brute-force by re-requesting).
    let existing = sqlx::query_as::<_, OtpChallenge>(
        "SELECT id, purpose, user_id, pending_email, pending_password_hash, pending_name,
                phone_number, code_hash, attempts, max_attempts, expires_at, consumed_at,
                last_sent_at, created_at
           FROM otp_challenges
          WHERE phone_number = $1 AND purpose = $2 AND consumed_at IS NULL
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(new.phone_number)
    .bind(new.purpose)
    .fetch_optional(db)
    .await?;

    if let Some(challenge) = existing {
        let since_last = now - challenge.last_sent_at;
        if since_last < Duration::seconds(RESEND_COOLDOWN_SECS) {
            let retry_after_secs =
                (RESEND_COOLDOWN_SECS - since_last.num_seconds()).max(0) as u64;
            return Err(OtpError::RateLimited { retry_after_secs });
        }

        // Refresh the code in place, keeping the attempt counter intact.
        let code = generate_code();
        let code_hash = hash_code(hmac_secret, challenge.id, &code);
        sqlx::query(
            "UPDATE otp_challenges
                SET code_hash = $2, expires_at = $3, last_sent_at = $4
              WHERE id = $1",
        )
        .bind(challenge.id)
        .bind(&code_hash)
        .bind(now + Duration::seconds(CODE_TTL_SECS))
        .bind(now)
        .execute(db)
        .await?;
        return Ok((challenge.id, code));
    }

    // Brand-new challenge: enforce the hourly spam cap.
    let recent_sends: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM otp_challenges
          WHERE phone_number = $1 AND purpose = $2 AND consumed_at IS NULL
            AND created_at > $3",
    )
    .bind(new.phone_number)
    .bind(new.purpose)
    .bind(now - Duration::hours(1))
    .fetch_one(db)
    .await?;
    if recent_sends >= MAX_SENDS_PER_HOUR {
        return Err(OtpError::RateLimited {
            retry_after_secs: 3600,
        });
    }

    let id = Uuid::new_v4();
    let code = generate_code();
    let code_hash = hash_code(hmac_secret, id, &code);
    sqlx::query(
        "INSERT INTO otp_challenges
            (id, purpose, user_id, pending_email, pending_password_hash, pending_name,
             phone_number, code_hash, attempts, max_attempts, expires_at, last_sent_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 0, $9, $10, $11)",
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
    .bind(now + Duration::seconds(CODE_TTL_SECS))
    .bind(now)
    .execute(db)
    .await?;

    Ok((id, code))
}

async fn send_code(otp: &dyn OtpProvider, phone_number: &str, code: &str) -> Result<(), OtpError> {
    otp.send_code(phone_number, code)
        .await
        .map_err(|err| OtpError::SendFailed(err.to_string()))
}

/// Generates a uniformly random `CODE_DIGITS`-digit decimal code.
///
/// Uses rejection sampling over `u32` so every value in `0..CODE_KEYSpace` is
/// equally likely (a plain `% 1_000_000` would bias the low end).
fn generate_code() -> String {
    let mut rng = rand::thread_rng();
    let limit = u32::MAX - (u32::MAX % CODE_KEYSpace as u32);
    let value = loop {
        let candidate = rng.next_u32();
        if candidate < limit {
            break candidate % CODE_KEYSpace as u32;
        }
    };
    format!("{:0width$}", value, width = CODE_DIGITS as usize)
}

fn hash_code(hmac_secret: &str, challenge_id: Uuid, code: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(hmac_secret.as_bytes())
        .expect("HMAC accepts keys of any length");
    mac.update(challenge_id.as_bytes());
    mac.update(b":");
    mac.update(code.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn verify_code(hmac_secret: &str, challenge_id: Uuid, code: &str, expected_hash: &str) -> bool {
    let actual = hash_code(hmac_secret, challenge_id, code);
    // Constant-time comparison to avoid leaking timing information.
    actual.len() == expected_hash.len()
        && actual
            .bytes()
            .zip(expected_hash.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Documents the brute-force surface of the 6-digit OTP design (issue #1422).
    ///
    /// The code is 6 decimal digits → `10^6 = 1_000_000` possible values. An
    /// attacker who knows the public `challenge_id` gets at most `MAX_ATTEMPTS`
    /// guesses per challenge, so the per-challenge success probability is
    /// `MAX_ATTEMPTS / CODE_KEYSpace = 5 / 1_000_000 = 0.0005%`. That is below
    /// the accepted threshold of `0.001%` per challenge, and is only acceptable
    /// because the window is bounded by `CODE_TTL_SECS` (10 minutes) and login
    /// issuance is additionally gated by per-IP rate limiting. If any of those
    /// mitigations is removed, the digit count must be revisited.
    #[test]
    fn six_digit_otp_bruteforce_probability_is_within_threshold() {
        // Keyspace: 6 decimal digits.
        assert_eq!(CODE_DIGITS, 6);
        assert_eq!(CODE_KEYSpace, 1_000_000);

        // Attempt budget and TTL are the mitigations that make 6 digits safe.
        assert_eq!(MAX_ATTEMPTS, 5);
        assert_eq!(CODE_TTL_SECS, 600); // 10 minutes

        // Per-challenge success probability for an attacker who knows the id.
        let per_challenge = MAX_ATTEMPTS as f64 / CODE_KEYSpace as f64;
        let per_challenge_pct = per_challenge * 100.0;
        assert!(
            (per_challenge_pct - 0.0005).abs() < 1e-12,
            "expected 0.0005% per challenge, got {per_challenge_pct}%"
        );
        assert!(
            per_challenge_pct < ACCEPTABLE_PER_CHALLENGE_SUCCESS_PCT,
            "per-challenge success probability must stay below the accepted threshold"
        );
    }

    #[test]
    fn generate_code_is_six_decimal_digits() {
        for _ in 0..100 {
            let code = generate_code();
            assert_eq!(code.len(), CODE_DIGITS as usize);
            assert!(code.chars().all(|c| c.is_ascii_digit()));
        }
    }
}
