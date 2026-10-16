use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Merchant {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
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
