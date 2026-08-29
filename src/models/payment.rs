use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Payment {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub wallet_id: Uuid,
    pub wallet_address: String,
    pub tx_hash: String,
    pub amount_stroops: i64,
    pub asset: String,
    pub network: String,
    pub status: String,
    pub confirmations: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewPayment {
    pub merchant_id: Uuid,
    pub wallet_id: Uuid,
    pub wallet_address: String,
    pub tx_hash: String,
    pub amount_stroops: i64,
    pub asset: String,
    pub network: String,
}

#[derive(Debug, Clone, Copy)]
pub enum UpdatePaymentStatus {
    Verified,
    Confirmed,
    Failed,
}