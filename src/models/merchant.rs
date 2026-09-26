use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Merchant {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub email: Option<String>,
    pub email_notifications_enabled: bool,
    pub unsubscribe_token: Option<String>,
    pub created_at: DateTime<Utc>,
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
