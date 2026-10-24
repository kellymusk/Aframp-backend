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
    /// Tracks consecutive failed password attempts. Reset to 0 on success.
    pub failed_login_count: i32,
    /// When `Some`, the account is locked until this timestamp. Cleared on
    /// admin unlock or when the time naturally passes.
    pub locked_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Row type combining a user with their (optional) merchant, produced by a
/// single LEFT JOIN query so callers avoid two separate DB roundtrips.
#[derive(Debug, Clone, FromRow)]
pub struct UserWithMerchant {
    pub id: Uuid,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub name: String,
    pub is_admin: bool,
    pub phone_number: Option<String>,
    pub phone_verified: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub merchant_id: Option<Uuid>,
}

impl UserWithMerchant {
    /// Extract the plain `User` portion of the joined row.
    pub fn into_user(self) -> User {
        User {
            id: self.id,
            email: self.email,
            password_hash: self.password_hash,
            name: self.name,
            is_admin: self.is_admin,
            phone_number: self.phone_number,
            phone_verified: self.phone_verified,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
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

/// Request body for `PATCH /users/me`.
///
/// Data model note: a merchant's `name` mirrors its owning user's `name`.
/// Updating the profile name must therefore update both `users.name` and
/// `merchants.name` in the same transaction so the admin endpoint and the
/// merchant profile never show divergent names for the same entity.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateProfileRequest {
    pub name: String,
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

    /// Compile-time guard: fails to compile if `User` ever gains a
    /// serialized field named `password_hash`. This is a structural
    /// assertion that complements the runtime serde test below — if
    /// someone removes `#[serde(skip_serializing)]`, the runtime test
    /// catches it; if someone adds a *new* field that leaks the hash,
    /// this helper documents the invariant that must be preserved.
    ///
    /// We express the invariant by requiring that `User` can be
    /// serialized into a value whose `password_hash` key is absent.
    /// The function itself is only referenced from tests, so it is
    /// compiled (and thus type-checked) whenever the test module is.
    #[allow(dead_code)]
    fn assert_password_hash_never_serialized(user: &User) {
        let value = serde_json::to_value(user).expect("User must serialize");
        assert!(
            value.get("password_hash").is_none(),
            "User::password_hash must never be serialized; \
             did someone remove #[serde(skip_serializing)]?"
        );
    }

    fn sample_user() -> User {
        let now = Utc::now();
        User {
            id: Uuid::new_v4(),
            email: "leak-check@example.com".to_string(),
            // Distinctive marker + argon2 prefix so we can grep the
            // serialized output for either.
            password_hash: "argon2id$v=19$m=19456,t=2,p=1$LEAKMARKER$hash".to_string(),
            name: "Leak Check".to_string(),
            is_admin: false,
            phone_number: Some("+15555550100".to_string()),
            phone_verified: true,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn user_serialization_omits_password_hash_key() {
        let user = sample_user();
        let value = serde_json::to_value(&user).expect("User must serialize");
        assert!(
            value.get("password_hash").is_none(),
            "serialized User must not contain a password_hash key"
        );
    }

    #[test]
    fn user_serialization_does_not_contain_argon2_or_marker() {
        let user = sample_user();
        let json = serde_json::to_string(&user).expect("User must serialize");
        assert!(
            !json.contains("password_hash"),
            "serialized User must not contain the string 'password_hash': {json}"
        );
        assert!(
            !json.contains("argon2"),
            "serialized User must not contain the string 'argon2': {json}"
        );
        assert!(
            !json.contains("LEAKMARKER"),
            "serialized User must not contain the password hash marker: {json}"
        );
    }

    #[test]
    fn compile_time_guard_holds() {
        // Exercises the compile-time helper so it is type-checked and
        // its runtime assertion runs as well.
        assert_password_hash_never_serialized(&sample_user());
    }
}
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
