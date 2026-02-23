use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Error Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
pub enum ProcessingError {
    #[error("account verification failed: {reason}")]
    AccountVerificationFailed { reason: String },

    #[error("payment processing failed: {reason}")]
    PaymentProcessingFailed { reason: String },

    #[error("provider error: {provider} - {reason}")]
    ProviderError { provider: String, reason: String },

    #[error("amount mismatch: expected {expected}, got {actual}")]
    AmountMismatch { expected: String, actual: String },

    #[error("retry limit exceeded: {attempts} attempts made")]
    RetryLimitExceeded { attempts: u32 },

    #[error("refund failed: {reason}")]
    RefundFailed { reason: String },

    #[error("database error: {0}")]
    Database(String),

    #[error("stellar error: {0}")]
    Stellar(String),

    #[error("timeout: {0}")]
    Timeout(String),

    #[error("invalid state: {0}")]
    InvalidState(String),
}

// ---------------------------------------------------------------------------
// Request/Response Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillPaymentRequest {
    pub transaction_id: String,
    pub provider_code: String,  // "ekedc", "mtn", "dstv-compact", etc.
    pub account_number: String, // Meter, phone, smart card, etc.
    pub account_type: String,   // "prepaid", "postpaid", etc.
    pub bill_type: String,      // "electricity", "airtime", "data", "cable_tv"
    pub amount: i64,            // Amount in smallest unit (kobo, cents)
    pub phone_number: Option<String>,
    pub variation_code: Option<String>, // For data bundles, cable packages
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillPaymentResponse {
    pub provider_reference: String,
    pub token: Option<String>,
    pub status: String, // "completed", "pending", "processing"
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentStatus {
    pub provider_reference: String,
    pub status: String, // "pending", "completed", "failed"
    pub token: Option<String>,
    pub amount: i64,
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Account Verification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AccountInfo {
    pub account_number: String,
    pub customer_name: String,
    pub account_type: String, // "prepaid", "postpaid"
    pub status: String,       // "active", "inactive", "suspended"
    pub outstanding_balance: Option<f64>,
    pub additional_info: String, // JSON string for flexibility
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationRequest {
    pub provider_code: String,
    pub account_number: String,
    pub account_type: String,
    pub bill_type: String,
}

// ---------------------------------------------------------------------------
// Bill Transaction Extended Model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct BillTransaction {
    pub id: Uuid,
    pub transaction_id: Uuid,
    pub wallet_address: String,
    pub bill_type: String,
    pub provider_code: String,
    pub account_number: String,
    pub amount: i64,
    pub status: String, // Processing state
    pub provider_reference: Option<String>,
    pub token: Option<String>,
    pub provider_response: Option<String>, // JSON response from provider
    pub retry_count: i32,
    pub last_retry_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub refund_tx_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Processing States
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillProcessingState {
    /// Waiting for cNGN payment
    PendingPayment,
    /// cNGN received, waiting to be processed
    CngnReceived,
    /// Verifying account validity
    VerifyingAccount,
    /// Account invalid, flagged for refund
    AccountInvalid,
    /// Processing bill payment with provider
    ProcessingBill,
    /// Provider processing the payment
    ProviderProcessing,
    /// Payment completed successfully
    Completed,
    /// Retrying payment
    RetryScheduled,
    /// Provider payment failed
    ProviderFailed,
    /// Payment should be refunded
    RefundInitiated,
    /// Refund in progress
    RefundProcessing,
    /// Refund completed
    Refunded,
}

impl BillProcessingState {
    pub fn as_str(&self) -> &'static str {
        match self {
            BillProcessingState::PendingPayment => "pending_payment",
            BillProcessingState::CngnReceived => "cngn_received",
            BillProcessingState::VerifyingAccount => "verifying_account",
            BillProcessingState::AccountInvalid => "account_invalid",
            BillProcessingState::ProcessingBill => "processing_bill",
            BillProcessingState::ProviderProcessing => "provider_processing",
            BillProcessingState::Completed => "completed",
            BillProcessingState::RetryScheduled => "retry_scheduled",
            BillProcessingState::ProviderFailed => "provider_failed",
            BillProcessingState::RefundInitiated => "refund_initiated",
            BillProcessingState::RefundProcessing => "refund_processing",
            BillProcessingState::Refunded => "refunded",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending_payment" => Some(BillProcessingState::PendingPayment),
            "cngn_received" => Some(BillProcessingState::CngnReceived),
            "verifying_account" => Some(BillProcessingState::VerifyingAccount),
            "account_invalid" => Some(BillProcessingState::AccountInvalid),
            "processing_bill" => Some(BillProcessingState::ProcessingBill),
            "provider_processing" => Some(BillProcessingState::ProviderProcessing),
            "completed" => Some(BillProcessingState::Completed),
            "retry_scheduled" => Some(BillProcessingState::RetryScheduled),
            "provider_failed" => Some(BillProcessingState::ProviderFailed),
            "refund_initiated" => Some(BillProcessingState::RefundInitiated),
            "refund_processing" => Some(BillProcessingState::RefundProcessing),
            "refunded" => Some(BillProcessingState::Refunded),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Retry Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub backoff_seconds: Vec<u64>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            backoff_seconds: vec![10, 60, 300],
        }
    }
}

// ---------------------------------------------------------------------------
// Token Management
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRetrievalResponse {
    pub token: Option<String>,
    pub status: String,
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Notification Data
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillPaymentNotification {
    pub transaction_id: String,
    pub bill_type: String,
    pub amount: i64,
    pub currency: String,
    pub account_number: String,
    pub provider: String,
    pub token: Option<String>,
    pub status: String,
    pub message: String,
    pub customer_email: Option<String>,
    pub customer_phone: Option<String>,
}
