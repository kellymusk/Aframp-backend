use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// Number of consecutive failed password attempts allowed before an
/// account is locked. Mirrors the OTP lockout threshold.
pub const MAX_FAILED_LOGIN_ATTEMPTS: i32 = 10;

/// How long an account stays locked after exceeding
/// [`MAX_FAILED_LOGIN_ATTEMPTS`].
pub const ACCOUNT_LOCKOUT_MINUTES: i64 = 30;

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
    /// Consecutive failed password attempts since the last successful
    /// login. Reset to `0` on success.
    pub failed_login_count: i32,
    /// When set and in the future, the account is locked and password
    /// logins are rejected until this timestamp passes (or an admin
    /// unlocks it).
    pub locked_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl User {
    /// Whether the account is currently locked out from password login.
    pub fn is_locked(&self) -> bool {
        matches!(self.locked_until, Some(until) if until > Utc::now())
    }
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
