use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// Maximum number of characters allowed in a user or merchant name.
///
/// This mirrors the `CHECK (char_length(name) <= 100)` constraints added to
/// the `users` and `merchants` tables in `migrations/0009_name_length_check.sql`
/// and the application-level check in `validate_name`.
pub const MAX_NAME_LENGTH: usize = 100;

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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
    use sqlx::postgres::PgPoolOptions;

    /// Verifies the database itself rejects names longer than
    /// `MAX_NAME_LENGTH`, even when the application-level `validate_name`
    /// check is bypassed by inserting directly.
    #[sqlx::test]
    async fn db_rejects_user_name_over_max_length(pool: sqlx::PgPool) {
        let long_name = "a".repeat(MAX_NAME_LENGTH + 1);

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
            "database should reject a user name longer than {MAX_NAME_LENGTH} characters"
        );
    }

    /// Sanity check that a name at the boundary is still accepted.
    #[sqlx::test]
    async fn db_accepts_user_name_at_max_length(pool: sqlx::PgPool) {
        let name = "a".repeat(MAX_NAME_LENGTH);

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
            "database should accept a user name of exactly {MAX_NAME_LENGTH} characters"
        );
    }
}
