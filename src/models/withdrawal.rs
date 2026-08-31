use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Withdrawal {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub amount_stroops: i64,
    pub asset: String,
    pub status: String,
    pub provider: Option<String>,
    pub provider_reference: Option<String>,
    pub bank_code: Option<String>,
    pub account_number: Option<String>,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateWithdrawalRequest {
    pub amount_stroops: i64,
    pub asset: Option<String>,
    pub bank_code: String,
    pub account_number: String,
}

#[derive(Debug, Clone)]
pub struct NewWithdrawal {
    pub merchant_id: Uuid,
    pub amount_stroops: i64,
    pub asset: String,
    pub bank_code: String,
    pub account_number: String,
}
