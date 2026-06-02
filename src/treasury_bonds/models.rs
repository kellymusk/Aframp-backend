//! Data models for the tokenized treasury bond rails

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TokenizedBondInstrument {
    pub id: Uuid,
    pub isin: String,
    pub issuer_authority: String,
    pub instrument_name: String,
    pub currency: String,
    pub face_value: sqlx::types::BigDecimal,
    pub coupon_rate_bps: i32,
    pub maturity_at: DateTime<Utc>,
    pub auction_date: Option<DateTime<Utc>>,
    pub on_chain_asset_code: Option<String>,
    pub stellar_issuer: Option<String>,
    pub status: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct BondLedgerAllocation {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub bond_instrument_id: Uuid,
    pub fractional_units: sqlx::types::BigDecimal,
    pub purchase_price: sqlx::types::BigDecimal,
    pub accrued_yield: sqlx::types::BigDecimal,
    pub on_chain_token_hash: Option<String>,
    pub stellar_tx_hash: Option<String>,
    pub status: String,
    pub acquired_at: DateTime<Utc>,
    pub redeemed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AutomatedSweepPolicy {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub enabled: bool,
    pub min_sweep_threshold_ngn: sqlx::types::BigDecimal,
    pub max_portfolio_duration_days: i32,
    pub preferred_instrument_id: Option<Uuid>,
    pub last_sweep_at: Option<DateTime<Utc>>,
    pub next_sweep_at: Option<DateTime<Utc>>,
    pub sweep_interval_minutes: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct RegisterBondRequest {
    pub isin: String,
    pub issuer_authority: String,
    pub instrument_name: String,
    pub currency: Option<String>,
    pub face_value: String,
    pub coupon_rate_bps: i32,
    pub maturity_at: DateTime<Utc>,
    pub auction_date: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct AllocateBondRequest {
    pub tenant_id: Uuid,
    pub bond_instrument_id: Uuid,
    pub fractional_units: String,
    pub purchase_price: String,
}

#[derive(Debug, Deserialize)]
pub struct SweepPolicyRequest {
    pub tenant_id: Uuid,
    pub enabled: Option<bool>,
    pub min_sweep_threshold_ngn: Option<String>,
    pub max_portfolio_duration_days: Option<i32>,
    pub preferred_instrument_id: Option<Uuid>,
    pub sweep_interval_minutes: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct LiquidateRequest {
    pub allocation_id: Uuid,
    pub reason: Option<String>,
}
