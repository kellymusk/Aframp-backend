//! Data models for the regulatory filing pipeline

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text")]
pub enum ReportType {
    Ctr,
    Sar,
    LiquidityRatio,
    CrossBorderFlow,
}

impl std::fmt::Display for ReportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Ctr => "CTR",
            Self::Sar => "SAR",
            Self::LiquidityRatio => "LIQUIDITY_RATIO",
            Self::CrossBorderFlow => "CROSS_BORDER_FLOW",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReportStatus {
    Pending,
    Compiled,
    Transmitted,
    Filed,
    FailedRemission,
    PendingRetry,
}

impl std::fmt::Display for ReportStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "PENDING",
            Self::Compiled => "COMPILED",
            Self::Transmitted => "TRANSMITTED",
            Self::Filed => "FILED",
            Self::FailedRemission => "FAILED_REMISSION",
            Self::PendingRetry => "PENDING_RETRY",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RegulatoryReport {
    pub id: Uuid,
    pub report_type: String,
    pub tenant_id: Option<Uuid>,
    pub agency_tag: String,
    pub submission_tracking_no: Option<String>,
    pub payload_hash: String,
    pub schema_format: String,
    pub status: String,
    pub compiled_at: Option<DateTime<Utc>>,
    pub transmitted_at: Option<DateTime<Utc>>,
    pub filed_at: Option<DateTime<Utc>>,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub retry_count: i32,
    pub error_detail: Option<String>,
    pub payload_bytes: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AgencyGateway {
    pub id: Uuid,
    pub agency_tag: String,
    pub display_name: String,
    pub jurisdiction: String,
    pub endpoint_url: String,
    pub protocol: String,
    pub auth_method: String,
    pub credentials_ref: Option<String>,
    pub public_key_pem: Option<String>,
    pub tls_version: String,
    pub is_active: bool,
    pub last_ping_at: Option<DateTime<Utc>>,
    pub last_ping_status: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AuditFilingEvent {
    pub id: Uuid,
    pub regulatory_report_id: Uuid,
    pub gateway_id: Option<Uuid>,
    pub event_type: String,
    pub http_status: Option<i32>,
    pub ack_code: Option<String>,
    pub nack_reason: Option<String>,
    pub payload_hash: Option<String>,
    pub duration_ms: Option<i32>,
    pub actor: String,
    pub raw_response: Option<serde_json::Value>,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateReportRequest {
    pub report_type: String,
    pub tenant_id: Option<Uuid>,
    pub agency_tag: String,
    pub schema_format: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RetryRequest {
    pub force: Option<bool>,
}
