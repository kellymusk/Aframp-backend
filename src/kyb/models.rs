//! KYB (Know Your Business) — Domain Models

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Workflow Status ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text")]
#[serde(rename_all = "snake_case")]
pub enum KybStatus {
    Draft,
    DocumentsSubmitted,
    RegistryVerified,
    ComplianceReview,
    Approved,
    Rejected,
}

impl std::fmt::Display for KybStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Draft => write!(f, "draft"),
            Self::DocumentsSubmitted => write!(f, "documents_submitted"),
            Self::RegistryVerified => write!(f, "registry_verified"),
            Self::ComplianceReview => write!(f, "compliance_review"),
            Self::Approved => write!(f, "approved"),
            Self::Rejected => write!(f, "rejected"),
        }
    }
}

// ── KYB Application ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct KybApplication {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub business_name: String,
    pub registration_number: String,
    pub jurisdiction: String,
    pub industry_code: Option<String>,
    pub registered_address: Option<String>,
    pub status: String,
    pub registry_status: Option<String>,
    pub registry_verified_at: Option<DateTime<Utc>>,
    pub registry_data: Option<serde_json::Value>,
    pub risk_level: Option<String>,
    pub risk_score: Option<f64>,
    pub reviewed_by: Option<String>,
    pub review_notes: Option<String>,
    pub rejection_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub approved_at: Option<DateTime<Utc>>,
}

// ── UBO ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Ubo {
    pub id: Uuid,
    pub kyb_application_id: Uuid,
    pub full_name: String,
    pub ownership_percentage: f64,
    pub nationality: Option<String>,
    pub date_of_birth: Option<NaiveDate>,
    pub kyc_user_id: Option<Uuid>,
    pub kyc_status: Option<String>,
    pub created_at: DateTime<Utc>,
}

// ── Document ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "text")]
#[serde(rename_all = "snake_case")]
pub enum DocumentType {
    Memorandum,
    Articles,
    ProofOfAddress,
    TaxCertificate,
    BankStatement,
    Other,
}

impl std::fmt::Display for DocumentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Memorandum => write!(f, "memorandum"),
            Self::Articles => write!(f, "articles"),
            Self::ProofOfAddress => write!(f, "proof_of_address"),
            Self::TaxCertificate => write!(f, "tax_certificate"),
            Self::BankStatement => write!(f, "bank_statement"),
            Self::Other => write!(f, "other"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct KybDocument {
    pub id: Uuid,
    pub kyb_application_id: Uuid,
    pub document_type: String,
    pub file_name: String,
    pub file_path: String,
    pub file_hash: String,
    pub encrypted: bool,
    pub ocr_extracted_data: Option<serde_json::Value>,
    pub ocr_confidence: Option<f64>,
    pub uploaded_at: DateTime<Utc>,
}

// ── Registry Check ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RegistryCheck {
    pub id: Uuid,
    pub kyb_application_id: Uuid,
    pub registry_provider: String,
    pub registration_number: String,
    pub check_status: String,
    pub response_data: Option<serde_json::Value>,
    pub error_message: Option<String>,
    pub checked_at: DateTime<Utc>,
}

/// Normalised data returned from any registry provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntityData {
    pub registration_number: String,
    pub company_name: String,
    pub status: String, // "active", "inactive", "deregistered"
    pub registered_address: Option<String>,
    pub incorporation_date: Option<NaiveDate>,
    pub directors: Vec<String>,
    pub shareholders: Vec<ShareholderRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareholderRecord {
    pub name: String,
    pub ownership_percentage: f64,
}

// ── Risk Score ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct KybRiskScore {
    pub id: Uuid,
    pub kyb_application_id: Uuid,
    pub score: f64,
    pub risk_level: String,
    pub factors: serde_json::Value,
    pub calculated_at: DateTime<Utc>,
}

// ── API Request / Response Types ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct StartKybRequest {
    pub merchant_id: Uuid,
    pub business_name: String,
    pub registration_number: String,
    pub jurisdiction: String,
    pub industry_code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SubmitDocumentRequest {
    pub document_type: DocumentType,
    pub file_name: String,
    pub file_content_b64: String, // Base64-encoded file bytes
}

#[derive(Debug, Deserialize)]
pub struct ReviewDecisionRequest {
    pub approved: bool,
    pub notes: Option<String>,
    pub rejection_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct KybApplicationSummary {
    pub application: KybApplication,
    pub ubos: Vec<Ubo>,
    pub documents: Vec<KybDocument>,
    pub latest_risk_score: Option<KybRiskScore>,
}
