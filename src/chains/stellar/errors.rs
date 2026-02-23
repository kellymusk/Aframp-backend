use thiserror::Error;

pub type StellarResult<T> = Result<T, StellarError>;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum StellarError {
    #[error("Account not found: {address}")]
    AccountNotFound { address: String },

    #[error("Invalid Stellar address: {address}")]
    InvalidAddress { address: String },

    #[error("Network error: {message}")]
    NetworkError { message: String },

    #[error("Rate limit exceeded. Please try again later")]
    RateLimitError,

    #[error("Configuration error: {message}")]
    ConfigError { message: String },

    #[error("Health check failed: {message}")]
    HealthCheckError { message: String },

    #[error("Serialization error: {message}")]
    SerializationError { message: String },

    #[error("Timeout error: operation timed out after {seconds} seconds")]
    TimeoutError { seconds: u64 },

    #[error("Unexpected error: {message}")]
    UnexpectedError { message: String },

    #[error("Insufficient XLM. Available: {available}, Required: {required}")]
    InsufficientXlm { available: String, required: String },

    #[error("Trustline already exists for account {address} and asset {asset}")]
    TrustlineAlreadyExists { address: String, asset: String },

    #[error("Transaction failed: {message}")]
    TransactionFailed { message: String },

    #[error("Signing error: {message}")]
    SigningError { message: String },
}

#[allow(dead_code)]
impl StellarError {
    pub fn account_not_found(address: impl Into<String>) -> Self {
        Self::AccountNotFound {
            address: address.into(),
        }
    }

    pub fn invalid_address(address: impl Into<String>) -> Self {
        Self::InvalidAddress {
            address: address.into(),
        }
    }

    pub fn network_error(message: impl Into<String>) -> Self {
        Self::NetworkError {
            message: message.into(),
        }
    }

    pub fn config_error(message: impl Into<String>) -> Self {
        Self::ConfigError {
            message: message.into(),
        }
    }

    pub fn health_check_error(message: impl Into<String>) -> Self {
        Self::HealthCheckError {
            message: message.into(),
        }
    }

    pub fn serialization_error(message: impl Into<String>) -> Self {
        Self::SerializationError {
            message: message.into(),
        }
    }

    pub fn timeout_error(seconds: u64) -> Self {
        Self::TimeoutError { seconds }
    }

    pub fn unexpected_error(message: impl Into<String>) -> Self {
        Self::UnexpectedError {
            message: message.into(),
        }
    }

    pub fn insufficient_xlm(available: impl Into<String>, required: impl Into<String>) -> Self {
        Self::InsufficientXlm {
            available: available.into(),
            required: required.into(),
        }
    }

    pub fn trustline_already_exists(address: impl Into<String>, asset: impl Into<String>) -> Self {
        Self::TrustlineAlreadyExists {
            address: address.into(),
            asset: asset.into(),
        }
    }

    pub fn transaction_failed(message: impl Into<String>) -> Self {
        Self::TransactionFailed {
            message: message.into(),
        }
    }

    pub fn signing_error(message: impl Into<String>) -> Self {
        Self::SigningError {
            message: message.into(),
        }
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for StellarError {
    fn from(err: Box<dyn std::error::Error + Send + Sync>) -> Self {
        let err_str = err.to_string();
        if err_str.contains("404") {
            StellarError::AccountNotFound {
                address: "unknown".to_string(),
            }
        } else if err_str.contains("429") || err_str.contains("rate limit") {
            StellarError::RateLimitError
        } else {
            StellarError::network_error(format!("Stellar SDK error: {}", err_str))
        }
    }
}

impl From<reqwest::Error> for StellarError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            StellarError::timeout_error(0)
        } else {
            StellarError::network_error(format!("Request error: {}", err))
        }
    }
}

impl From<serde_json::Error> for StellarError {
    fn from(err: serde_json::Error) -> Self {
        StellarError::serialization_error(format!("JSON error: {}", err))
    }
}
