use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Wallet {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub address: String,
    pub network: String,
    pub created_at: DateTime<Utc>,
    pub last_polled_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewWallet {
    pub merchant_id: Uuid,
    pub address: String,
    pub network: String,
    pub secret_key_encrypted: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateWalletRequest {
    pub network: Option<String>,
}