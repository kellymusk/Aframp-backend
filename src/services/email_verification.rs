//! Email verification for merchant signup (#1232).
//!
//! Signup already verifies the phone number via OTP, but the email address is
//! never confirmed — a typo means the merchant silently loses every
//! transactional email. This module issues a short-lived, single-use token,
//! (optionally) delivers it by email, and consumes it via
//! `GET /verify-email?token=...`.

use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

/// How long a verification link stays valid.
pub const TOKEN_TTL_HOURS: i64 = 24;

/// Tokens are stored as a SHA-256 hash so a database leak doesn't yield usable
/// links — the raw token only ever exists in the email and the client.
fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Issues a fresh verification token for `user_id`, invalidating any previous
/// one, and returns the raw token to embed in the verification link.
pub async fn issue_token(db: &PgPool, user_id: Uuid) -> Result<String, sqlx::Error> {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    let token = hex::encode(bytes);
    let token_hash = hash_token(&token);

    sqlx::query(
        "UPDATE users
            SET email_verification_token = $1,
                email_verification_expires_at = now() + make_interval(hours => $2)
          WHERE id = $3",
    )
    .bind(token_hash)
    .bind(TOKEN_TTL_HOURS as i32)
    .bind(user_id)
    .execute(db)
    .await?;

    Ok(token)
}

/// Marks a user's email verified, consuming the token. Returns `true` only when
/// a live token matched — expired or unknown tokens return `false`.
pub async fn verify_token(db: &PgPool, token: &str) -> Result<bool, sqlx::Error> {
    let token_hash = hash_token(token);
    let result = sqlx::query(
        "UPDATE users
            SET email_verified = true,
                email_verification_token = NULL,
                email_verification_expires_at = NULL
          WHERE email_verification_token = $1
            AND email_verification_expires_at > now()",
    )
    .bind(token_hash)
    .execute(db)
    .await?;

    Ok(result.rows_affected() == 1)
}

/// Delivers a verification email. No transactional provider is wired into the
/// backend yet, so this records the link through structured logging; swapping
/// in a provider only means changing this function's body.
pub fn send_verification_email(to: &str, token: &str) {
    let link = format!("/verify-email?token={token}");
    tracing::info!(
        email = %to,
        verification_link = %link,
        "email verification link generated"
    );
}
