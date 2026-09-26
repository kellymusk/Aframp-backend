use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Merchant {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    /// `Some` when the merchant account is suspended. The timestamp records
    /// when the suspension was applied. `None` means the account is active.
    pub suspended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
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
