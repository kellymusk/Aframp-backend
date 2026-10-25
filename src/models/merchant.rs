use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum KycStatus {
    Unverified,
    Pending,
    Verified,
    Rejected,
}

impl Default for KycStatus {
    fn default() -> Self {
        KycStatus::Unverified
    }
}

#[derive(Debug, Clone, Serialize, FromRow)]
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Merchant {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub kyc_status: KycStatus,
    pub kyc_bvn: Option<String>,
    pub kyc_cac_number: Option<String>,
    pub kyc_submitted_at: Option<DateTime<Utc>>,
    pub kyc_reviewed_at: Option<DateTime<Utc>>,
    pub kyc_rejection_reason: Option<String>,
    pub email: Option<String>,
    pub email_notifications_enabled: bool,
    pub unsubscribe_token: Option<String>,
    /// `Some` when the merchant account is suspended. The timestamp records
    /// when the suspension was applied. `None` means the account is active.
    pub suspended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Merchant {
    /// Returns `true` if this merchant is currently suspended.
    pub fn is_suspended(&self) -> bool {
        self.suspended_at.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct NewMerchant {
    pub user_id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubmitKycRequest {
    pub bvn: Option<String>,
    pub cac_number: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdminKycDecisionRequest {
    pub status: KycStatus,
    pub rejection_reason: Option<String>,
}
    pub email: Option<String>,
}

impl Merchant {
    /// Whether this merchant should receive proactive deposit notifications.
    /// Notifications are opt-in: they require an email address, an explicit
    /// preference flag, and a token used for GDPR-compliant unsubscribe links.
    pub fn wants_deposit_notifications(&self) -> bool {
        self.email_notifications_enabled
            && self.email.as_deref().map_or(false, |e| !e.trim().is_empty())
            && self.unsubscribe_token.is_some()
    }
}
