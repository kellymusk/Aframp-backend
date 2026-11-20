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

    // The signup branch must not lose the challenge if account creation
    // fails: the challenge holds the pending password hash, so deleting it
    // before `create_verified` succeeds would strand the user with a used
    // code and no account. Claim the challenge and create the user inside a
    // single transaction so a failure rolls back the DELETE and the
    // challenge stays claimable.
    if challenge.purpose == "signup" {
        let mut tx = db.begin().await?;

        let claimed = sqlx::query("DELETE FROM otp_challenges WHERE id = $1")
            .bind(challenge_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if claimed == 0 {
            // Another verify already claimed it; nothing to roll back.
            tx.rollback().await?;
            return Err(OtpError::ChallengeNotFound);
        }

        let email = challenge
            .pending_email
            .as_deref()
            .expect("signup challenge always carries pending_email — enforced by the migration's CHECK constraint");
        let password_hash = challenge
            .pending_password_hash
            .as_deref()
            .expect("signup challenge always carries pending_password_hash — enforced by the migration's CHECK constraint");
        let name = challenge
            .pending_name
            .as_deref()
            .expect("signup challenge always carries pending_name — enforced by the migration's CHECK constraint");

        let (user, merchant) = users::create_verified(
            &mut tx,
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

        tx.commit().await?;
        return Ok(VerifiedOutcome::Signup(user, merchant));
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

    Err(OtpError::ChallengeNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for #1411: if `create_verified` fails mid-signup, the
    /// challenge DELETE must roll back so the user can retry with the same
    /// code instead of being stranded with a used OTP and no account.
    ///
    /// Requires a live Postgres (`DATABASE_URL`); skipped otherwise.
    #[tokio::test]
    async fn signup_verify_keeps_challenge_when_create_fails() {
        let url = match std::env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => return,
        };
        let db = match PgPool::connect(&url).await {
            Ok(pool) => pool,
            Err(_) => return,
        };

        let hmac_secret = "test-secret";
        let challenge_id = Uuid::new_v4();
        let code = "123456";
        let phone = format!("+1555{:07}", rand::random::<u32>() % 10_000_000);
        let email = format!("otp-1411-{}@example.com", challenge_id);

        // Seed a signup challenge whose pending email collides with an
        // existing user, so `create_verified` fails on the unique constraint
        // *after* the DELETE has run inside the transaction.
        sqlx::query(
            "INSERT INTO otp_challenges
                (id, purpose, user_id, pending_email, pending_password_hash, pending_name,
                 phone_number, code_hash, attempts, max_attempts, expires_at, consumed_at,
                 last_sent_at, created_at)
             VALUES ($1, 'signup', NULL, $2, 'hash', 'Name', $3, $4, 0, 5,
                     now() + interval '10 minutes', NULL, now(), now())",
        )
        .bind(challenge_id)
        .bind(&email)
        .bind(&phone)
        .bind(verify_code(hmac_secret, challenge_id, code, ""))
        .execute(&db)
        .await
        .expect("seed challenge");

        // Force the failure: pre-create a user with the same email.
        sqlx::query(
            "INSERT INTO users (email, password_hash, name, phone_number, phone_verified)
             VALUES ($1, 'hash', 'Existing', $2, true)",
        )
        .bind(&email)
        .bind(format!("{}-existing", phone))
        .execute(&db)
        .await
        .expect("seed colliding user");

        let result = verify(&db, hmac_secret, challenge_id, code).await;
        assert!(matches!(result, Err(OtpError::EmailTaken)));

        // The challenge must still be present and claimable.
        let still_there: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM otp_challenges WHERE id = $1)")
                .bind(challenge_id)
                .fetch_one(&db)
                .await
                .expect("check challenge");
        assert!(still_there, "challenge must survive a failed signup");

        // Cleanup.
        let _ = sqlx::query("DELETE FROM otp_challenges WHERE id = $1")
            .bind(challenge_id)
            .execute(&db)
            .await;
        let _ = sqlx::query("DELETE FROM users WHERE email = $1")
            .bind(&email)
            .execute(&db)
            .await;
    }
}
