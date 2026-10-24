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

impl OtpChallenge {
    /// A challenge is "fresh" (still counts toward the hourly spam cap) only
    /// while it has not been successfully consumed. Verified challenges must
    /// not lock a merchant out of requesting a new OTP.
    pub fn is_fresh(&self) -> bool {
        self.consumed_at.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn challenge(consumed_at: Option<DateTime<Utc>>) -> OtpChallenge {
        let now = Utc::now();
        OtpChallenge {
            id: Uuid::new_v4(),
            purpose: "login".to_string(),
            user_id: None,
            pending_email: None,
            pending_password_hash: None,
            pending_name: None,
            phone_number: "+15555550100".to_string(),
            code_hash: "hash".to_string(),
            attempts: 0,
            max_attempts: 5,
            expires_at: now + Duration::minutes(10),
            consumed_at,
            last_sent_at: now,
            created_at: now,
        }
    }

    #[test]
    fn consumed_challenges_are_not_fresh() {
        let now = Utc::now();
        let consumed = challenge(Some(now));
        let unconsumed = challenge(None);

        // Only unconsumed challenges should count toward the hourly cap, so a
        // merchant with 5 verified challenges is not locked out.
        assert!(!consumed.is_fresh());
        assert!(unconsumed.is_fresh());

        let all = [consumed, unconsumed];
        let counted = all.iter().filter(|c| c.is_fresh()).count();
        assert_eq!(counted, 1);
        assert_eq!(all.len(), 2);
    }
}
