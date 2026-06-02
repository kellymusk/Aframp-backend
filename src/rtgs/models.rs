//! Data models for the RTGS interbank settlement rail

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettlementStatus {
    Pending,
    Settled,
    Reversed,
    HeldForReconciliation,
    Failed,
}

impl std::fmt::Display for SettlementStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "PENDING",
            Self::Settled => "SETTLED",
            Self::Reversed => "REVERSED",
            Self::HeldForReconciliation => "HELD_FOR_RECONCILIATION",
            Self::Failed => "FAILED",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TwoPcPhase {
    None,
    Prepare,
    Commit,
    Abort,
}

impl std::fmt::Display for TwoPcPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::None => "NONE",
            Self::Prepare => "PREPARE",
            Self::Commit => "COMMIT",
            Self::Abort => "ABORT",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RtgsSettlementPool {
    pub id: Uuid,
    pub bank_code: String,
    pub bank_name: String,
    pub currency: String,
    pub available_limit: sqlx::types::BigDecimal,
    pub net_debit_cap: sqlx::types::BigDecimal,
    pub clearing_account_ref: String,
    pub is_active: bool,
    pub last_settlement_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ClearingHouseLedgerEntry {
    pub id: Uuid,
    pub settlement_pool_id: Uuid,
    pub on_chain_tx_hash: Option<String>,
    pub stellar_ledger_sequence: Option<i64>,
    pub bank_tracking_ref: String,
    pub amount: sqlx::types::BigDecimal,
    pub currency: String,
    pub direction: String,
    pub status: String,
    pub two_pc_phase: String,
    pub hsm_signature: Option<String>,
    pub aml_metadata: serde_json::Value,
    pub settled_at: Option<DateTime<Utc>>,
    pub reversed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct InterbankReconciliationLog {
    pub id: Uuid,
    pub ledger_entry_id: Uuid,
    pub ack_code: Option<String>,
    pub nack_reason: Option<String>,
    pub message_type: String,
    pub iso20022_payload: Option<serde_json::Value>,
    pub processing_node: Option<String>,
    pub duration_ms: Option<i32>,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSettlementRequest {
    pub bank_code: String,
    pub bank_tracking_ref: String,
    pub amount: String,
    pub currency: Option<String>,
    pub direction: String,
    pub aml_metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct CommitSettlementRequest {
    pub stellar_tx_hash: Option<String>,
    pub stellar_ledger_sequence: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ReverseSettlementRequest {
    pub reason: String,
}

#[derive(Debug, Deserialize)]
pub struct RegisterPoolRequest {
    pub bank_code: String,
    pub bank_name: String,
    pub currency: Option<String>,
    pub available_limit: String,
    pub net_debit_cap: String,
    pub clearing_account_ref: String,
}
