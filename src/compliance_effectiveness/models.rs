//! Compliance Effectiveness Reporting — Data Models

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Report Type ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text")]
#[serde(rename_all = "snake_case")]
pub enum ReportType {
    Monthly,
    Quarterly,
    Annual,
    AdHoc,
}

impl std::fmt::Display for ReportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Monthly => write!(f, "monthly"),
            Self::Quarterly => write!(f, "quarterly"),
            Self::Annual => write!(f, "annual"),
            Self::AdHoc => write!(f, "ad_hoc"),
        }
    }
}

impl std::str::FromStr for ReportType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "monthly" => Ok(Self::Monthly),
            "quarterly" => Ok(Self::Quarterly),
            "annual" => Ok(Self::Annual),
            "ad_hoc" => Ok(Self::AdHoc),
            _ => Err(format!("Unknown report type: {s}")),
        }
    }
}

// ── Report Format ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text")]
#[serde(rename_all = "lowercase")]
pub enum ReportFormat {
    Pdf,
    Csv,
    Json,
}

impl std::fmt::Display for ReportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pdf => write!(f, "pdf"),
            Self::Csv => write!(f, "csv"),
            Self::Json => write!(f, "json"),
        }
    }
}

// ── Trend Direction ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrendDirection {
    Increasing,
    Decreasing,
    Stable,
}

impl std::fmt::Display for TrendDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Increasing => write!(f, "increasing"),
            Self::Decreasing => write!(f, "decreasing"),
            Self::Stable => write!(f, "stable"),
        }
    }
}

// ── Aggregated KPI Metrics ────────────────────────────────────────────────────

/// Raw KPI data aggregated from aml_cases and related tables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceMetrics {
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,

    // Alert volume
    pub total_alerts: i64,
    pub sanctions_alerts: i64,
    pub aml_alerts: i64,
    pub kyc_alerts: i64,

    // False positive analysis
    pub false_positives: i64,
    pub false_positive_rate: f64,

    // SLA / resolution time
    pub avg_resolution_time_hrs: f64,
    pub median_resolution_time_hrs: f64,
    pub sla_breaches: i64,
    pub sla_compliance_rate: f64,

    // Case disposition
    pub cases_cleared: i64,
    pub cases_blocked: i64,
    pub cases_pending: i64,

    // Risk distribution
    pub low_risk_cases: i64,
    pub medium_risk_cases: i64,
    pub critical_risk_cases: i64,

    // Trend analysis (compared to previous period)
    pub alert_volume_trend: Option<TrendDirection>,
    pub false_positive_trend: Option<TrendDirection>,
}

// ── Persisted Report ──────────────────────────────────────────────────────────

/// A compliance effectiveness report stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ComplianceReport {
    pub id: Uuid,
    pub report_type: String,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,

    pub total_alerts: i32,
    pub sanctions_alerts: i32,
    pub aml_alerts: i32,
    pub kyc_alerts: i32,

    pub false_positives: i32,
    pub false_positive_rate: f64,

    pub avg_resolution_time_hrs: f64,
    pub median_resolution_time_hrs: f64,
    pub sla_breaches: i32,
    pub sla_compliance_rate: f64,

    pub cases_cleared: i32,
    pub cases_blocked: i32,
    pub cases_pending: i32,

    pub low_risk_cases: i32,
    pub medium_risk_cases: i32,
    pub critical_risk_cases: i32,

    pub alert_volume_trend: Option<String>,
    pub false_positive_trend: Option<String>,

    pub generated_by: String,
    pub generated_at: DateTime<Utc>,
    pub format: String,
    pub file_path: Option<String>,
    pub created_at: DateTime<Utc>,
}

// ── Report Schedule ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ReportSchedule {
    pub id: Uuid,
    pub schedule_name: String,
    pub report_type: String,
    pub cron_expression: String,
    pub format: String,
    pub recipients: Vec<String>,
    pub enabled: bool,
    pub last_run_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ── API Request / Response Types ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GenerateReportRequest {
    pub report_type: ReportType,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub format: ReportFormat,
}

#[derive(Debug, Deserialize)]
pub struct ListReportsQuery {
    pub report_type: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

impl ListReportsQuery {
    pub fn page(&self) -> i64 { self.page.unwrap_or(1).max(1) }
    pub fn page_size(&self) -> i64 { self.page_size.unwrap_or(20).clamp(1, 100) }
    pub fn offset(&self) -> i64 { (self.page() - 1) * self.page_size() }
}

#[derive(Debug, Serialize)]
pub struct ReportListPage {
    pub reports: Vec<ComplianceReport>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}
