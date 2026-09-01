use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Balance {
    pub merchant_id: Uuid,
    pub asset: String,
    pub available: i64,
    pub pending: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UpdateBalance {
    pub merchant_id: Uuid,
    pub asset: String,
    pub available_delta: i64,
    pub pending_delta: i64,
}
