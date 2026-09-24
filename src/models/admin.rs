use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminUserRow {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub merchant_id: Option<Uuid>,
    pub merchant_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminMerchantRow {
    pub id: Uuid,
    pub name: String,
    pub owner_user_id: Uuid,
    pub owner_email: String,
    pub created_at: DateTime<Utc>,
    pub wallet_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminWalletRow {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub merchant_name: String,
    pub address: String,
    pub network: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminTransactionRow {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub merchant_name: String,
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

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminWithdrawalRow {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub merchant_name: String,
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

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminPaymentRequestRow {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub merchant_name: String,
    pub amount_stroops: i64,
    pub asset: String,
    pub memo: String,
    pub status: String,
    pub payment_id: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AssetTotal {
    pub asset: String,
    pub available: i64,
    pub pending: i64,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct StatusCount {
    pub status: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdminOverview {
    pub total_users: i64,
    pub total_merchants: i64,
    pub total_wallets: i64,
    pub balances_by_asset: Vec<AssetTotal>,
    pub payments_by_status: Vec<StatusCount>,
    pub withdrawals_by_status: Vec<StatusCount>,
    pub payment_requests_by_status: Vec<StatusCount>,
}
