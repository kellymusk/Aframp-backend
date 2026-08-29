/// Error types for Stellar submission engine
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SubmissionError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("Horizon API error: {0}")]
    HorizonApi(String),

    #[error("Bad sequence number: {0}")]
    BadSequence(String),

    #[error("Insufficient fee: fee {provided} stroops required, minimum {required} stroops")]
    InsufficientFee { provided: i64, required: i64 },

    #[error("Transaction malformed: {0}")]
    MalformedTransaction(String),

    #[error("No active channels available")]
    NoActiveChannels,

    #[error("Channel exhausted: {0}")]
    ChannelExhausted(String),

    #[error("Sequence coordinator error: {0}")]
    SequenceCoordinatorError(String),

    #[error("Transient network error: {source} (retry attempt {attempt})")]
    TransientNetworkError { source: String, attempt: u32 },

    #[error("Max retries exceeded: {0}")]
    MaxRetriesExceeded(String),

    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    #[error("Invalid transaction envelope: {0}")]
    InvalidEnvelope(String),

    #[error("Fee calculation error: {0}")]
    FeeCalculationError(String),

    #[error("Metrics error: {0}")]
    MetricsError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Ledger close timeout after {attempts} retries")]
    LedgerCloseTimeout { attempts: u32 },

    #[error("Unknown Horizon error: {code}: {message}")]
    UnknownHorizonError { code: String, message: String },

    #[error("Channel rotation error: {0}")]
    ChannelRotationError(String),
}

pub type SubmissionResult<T> = Result<T, SubmissionError>;

/// Horizon-specific error codes for classification
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HorizonErrorCode {
    /// Transaction has bad sequence number (account sequence mismatch)
    TxBadSeq,
    /// Insufficient base reserve or transaction fee
    TxInsufficientFee,
    /// Transaction structure is invalid
    TxMalformed,
    /// Generic stale ledger error
    StaleLedgerVersion,
    /// Node is not in sync
    InternalServerError,
    /// Generic transient error
    Transient,
    /// Unknown error
    Unknown(String),
}

impl HorizonErrorCode {
    /// Parse Horizon error code from string
    pub fn from_str(s: &str) -> Self {
        match s {
            "tx_bad_seq" => HorizonErrorCode::TxBadSeq,
            "tx_insufficient_fee" => HorizonErrorCode::TxInsufficientFee,
            "tx_malformed" => HorizonErrorCode::TxMalformed,
            "stale_ledger_version" => HorizonErrorCode::StaleLedgerVersion,
            "internal_server_error" | "500" => HorizonErrorCode::InternalServerError,
            s if s.contains("timeout") || s.contains("connection") => HorizonErrorCode::Transient,
            other => HorizonErrorCode::Unknown(other.to_string()),
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            HorizonErrorCode::Transient
                | HorizonErrorCode::InternalServerError
                | HorizonErrorCode::StaleLedgerVersion
        )
    }

    pub fn is_channel_exhaustion(&self) -> bool {
        matches!(
            self,
            HorizonErrorCode::TxBadSeq | HorizonErrorCode::TxInsufficientFee
        )
    }
}

impl From<sqlx::Error> for SubmissionError {
    fn from(err: sqlx::Error) -> Self {
        SubmissionError::Database(err.to_string())
    }
}

impl From<serde_json::Error> for SubmissionError {
    fn from(err: serde_json::Error) -> Self {
        SubmissionError::SerializationError(err.to_string())
    }
}
