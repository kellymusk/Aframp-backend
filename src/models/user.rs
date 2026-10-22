use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub name: String,
    pub is_admin: bool,
    /// `None` only for accounts created before phone/OTP verification
    /// existed — they keep logging in without a challenge. Every account
    /// created since always has one.
    pub phone_number: Option<String>,
    pub phone_verified: bool,
    /// Tracks consecutive failed password attempts. Reset to 0 on success.
    pub failed_login_count: i32,
    /// When `Some`, the account is locked until this timestamp. Cleared on
    /// admin unlock or when the time naturally passes.
    pub locked_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Safe read model for API responses that should never expose password hashes.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct UserProfile {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SignupRequest {
    pub email: String,
    pub password: String,
    pub name: String,
    pub phone_number: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user_id: Uuid,
    pub merchant_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct NewUser {
    pub email: String,
    pub password_hash: String,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::MAX_NAME_LEN;

    fn sample_user() -> User {
        let now = Utc::now();
        User {
            id: Uuid::new_v4(),
            email: "leak-check@example.com".to_string(),
            // Distinctive marker + argon2 prefix so a leak is easy to spot.
            password_hash: "argon2id$v=19$m=19456,t=2,p=1$LEAKMARKER$hash".to_string(),
            name: "Leak Check".to_string(),
            is_admin: false,
            phone_number: Some("+2348011122233".to_string()),
            phone_verified: true,
            failed_login_count: 0,
            locked_until: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn user_serialization_never_includes_the_password_hash() {
        let json = serde_json::to_string(&sample_user()).expect("User must serialize");
        assert!(!json.contains("password_hash"), "{json}");
        assert!(!json.contains("argon2"), "{json}");
        assert!(!json.contains("LEAKMARKER"), "{json}");
    }

    /// Verifies the database itself rejects names longer than
    /// `MAX_NAME_LEN`, even when the application-level `validate_name`
    /// check is bypassed by inserting directly.
    #[sqlx::test]
    async fn db_rejects_user_name_over_max_length(pool: sqlx::PgPool) {
        let long_name = "a".repeat(MAX_NAME_LEN + 1);

        let result = sqlx::query(
            "INSERT INTO users (id, email, password_hash, name, is_admin, phone_verified, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, false, false, now(), now())",
        )
        .bind(Uuid::new_v4())
        .bind("too-long-name@example.com")
        .bind("hash")
        .bind(&long_name)
        .execute(&pool)
        .await;

        assert!(
            result.is_err(),
            "database should reject a user name longer than {MAX_NAME_LEN} characters"
        );
    }

    /// Sanity check that a name at the boundary is still accepted.
    #[sqlx::test]
    async fn db_accepts_user_name_at_max_length(pool: sqlx::PgPool) {
        let name = "a".repeat(MAX_NAME_LEN);

        let result = sqlx::query(
            "INSERT INTO users (id, email, password_hash, name, is_admin, phone_verified, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, false, false, now(), now())",
        )
        .bind(Uuid::new_v4())
        .bind("boundary-name@example.com")
        .bind("hash")
        .bind(&name)
        .execute(&pool)
        .await;

        assert!(
            result.is_ok(),
            "database should accept a user name of exactly {MAX_NAME_LEN} characters"
        );
    }
}
