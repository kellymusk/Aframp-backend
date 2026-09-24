use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize)]
pub struct VerifyOtpRequest {
    pub challenge_id: Uuid,
    pub code: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OtpChallengeResponse {
    pub challenge_id: Uuid,
    pub expires_in_secs: i64,
}

/// The `otp_challenges` row shape used internally by `services::otp`. Both
/// `purpose`s share this struct; the CHECK constraint in migration 0008
/// enforces which fields are populated for which purpose, not this type.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OtpChallenge {
    pub id: Uuid,
    pub purpose: String,
    pub user_id: Option<Uuid>,
    pub pending_email: Option<String>,
    pub pending_password_hash: Option<String>,
    pub pending_name: Option<String>,
    pub phone_number: String,
    pub code_hash: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub last_sent_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}
