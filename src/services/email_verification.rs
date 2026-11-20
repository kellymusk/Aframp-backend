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

/// Delivers a verification email. No transactional email provider is wired
/// into the backend yet, so for now this only records that a link was issued.
/// Neither the token nor the recipient address is logged: the token is a
/// single-use credential and the email is PII, and logs are readable by far
/// more people than the account owner.
pub fn send_verification_email(to: &str, _token: &str) {
    // `to` is intentionally not logged — it is PII (#1420).
    let _ = to;
    tracing::info!("email verification link issued (no email provider configured)");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    /// Minimal subscriber that captures the rendered `message` field of every
    /// event so tests can assert on what actually reaches the log stream.
    #[derive(Clone, Default)]
    struct CaptureSubscriber {
        messages: Arc<Mutex<Vec<String>>>,
    }

    struct MessageVisitor<'a>(&'a mut String);

    impl<'a> Visit for MessageVisitor<'a> {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                *self.0 = format!("{:?}", value);
            }
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "message" {
                *self.0 = value.to_string();
            }
        }
    }

    impl Subscriber for CaptureSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut message = String::new();
            event.record(&mut MessageVisitor(&mut message));
            self.messages.lock().unwrap().push(message);
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    #[test]
    fn send_verification_email_does_not_log_recipient_address() {
        let subscriber = CaptureSubscriber::default();
        let messages = subscriber.messages.clone();

        tracing::subscriber::with_default(subscriber, || {
            send_verification_email("merchant@example.com", "secret-token");
        });

        let captured = messages.lock().unwrap();
        assert!(!captured.is_empty(), "expected a log line to be emitted");
        for line in captured.iter() {
            assert!(
                !line.contains("merchant@example.com"),
                "log line leaked recipient email: {line}"
            );
            assert!(
                !line.contains("secret-token"),
                "log line leaked verification token: {line}"
            );
        }
    }
}
