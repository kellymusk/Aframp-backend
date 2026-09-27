use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Merchant {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub suspended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl Merchant {
    /// A merchant is suspended when a suspension timestamp has been recorded.
    pub fn is_suspended(&self) -> bool {
        self.suspended_at.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct NewMerchant {
    pub user_id: Uuid,
    pub name: String,
}