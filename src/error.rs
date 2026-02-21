//! Comprehensive error handling for Aframp backend
//!
//! This module provides a unified error system with proper HTTP status mapping,
//! user-friendly messages, and structured error codes for client handling.

#[cfg(feature = "database")]
use serde::{Deserialize, Serialize};
use std::fmt;

#[cfg(feature = "database")]
use crate::chains::stellar::errors::StellarError;

/// CNGN-specific error codes for programmatic handling
#[cfg(feature = "database")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCode {
    // Domain errors (4xx)
    #[serde(rename = "TRUSTLINE_REQUIRED")]
    TrustlineRequired,
    #[serde(rename = "INSUFFICIENT_CNGN_BALANCE")]
    InsufficientCngnBalance,
    #[serde(rename = "RATE_EXPIRED")]
    RateExpired,
    #[serde(rename = "INVALID_CNGN_AMOUNT")]
    InvalidCngnAmount,
    #[serde(rename = "TRUSTLINE_CREATION_FAILED")]
    TrustlineCreationFailed,
    #[serde(rename = "TRANSACTION_NOT_FOUND")]
    TransactionNotFound,
    #[serde(rename = "WALLET_NOT_FOUND")]
    WalletNotFound,
    #[serde(rename = "INVALID_WALLET_ADDRESS")]
    InvalidWalletAddress,
    #[serde(rename = "INVALID_CURRENCY")]
    InvalidCurrency,
    #[serde(rename = "INVALID_AMOUNT")]
    InvalidAmount,
    #[serde(rename = "AMOUNT_TOO_LOW")]
    AmountTooLow,
    #[serde(rename = "INSUFFICIENT_LIQUIDITY")]
    InsufficientLiquidity,
    #[serde(rename = "INVALID_WALLET")]
    InvalidWallet,
    #[serde(rename = "DUPLICATE_TRANSACTION")]
    DuplicateTransaction,

    // Infrastructure errors (5xx)
    #[serde(rename = "DATABASE_ERROR")]
    DatabaseError,
    #[serde(rename = "CACHE_ERROR")]
    CacheError,
    #[serde(rename = "CONFIGURATION_ERROR")]
    ConfigurationError,

    // External errors (502, 503, 504)
    #[serde(rename = "PAYMENT_PROVIDER_ERROR")]
    PaymentProviderError,
    #[serde(rename = "BLOCKCHAIN_ERROR")]
    BlockchainError,
    #[serde(rename = "RATE_LIMIT_ERROR")]
    RateLimitError,
    #[serde(rename = "EXTERNAL_SERVICE_TIMEOUT")]
    ExternalServiceTimeout,

    // Generic
    #[serde(rename = "INTERNAL_ERROR")]
    InternalError,
    #[serde(rename = "VALIDATION_ERROR")]
    ValidationError,
}

/// Domain-specific business logic errors
#[cfg(feature = "database")]
#[derive(Debug, Clone)]
pub enum DomainError {
    /// User doesn't have enough CNGN tokens for the operation
    InsufficientBalance { available: String, required: String },
    /// Wallet hasn't established CNGN trustline
    TrustlineNotFound {
        wallet_address: String,
        asset: String,
    },
    /// Amount is invalid (negative, zero, or out of range)
    InvalidAmount { amount: String, reason: String },
    /// Amount below minimum threshold
    AmountTooLow { amount: String, minimum: String },
    /// Transaction with given ID doesn't exist
    TransactionNotFound { transaction_id: String },
    /// Wallet doesn't exist in the system
    WalletNotFound { wallet_address: String },
    /// Exchange rate quote has expired
    RateExpired { quote_id: String },
    /// Duplicate transaction attempt
    DuplicateTransaction { transaction_id: String },
    /// Failed to create trustline on Stellar
    TrustlineCreationFailed {
        wallet_address: String,
        reason: String,
    },
    /// Insufficient cNGN liquidity on Stellar for onramp
    InsufficientLiquidity { amount: String },
}

/// Infrastructure-level errors (database, cache, configuration)
#[cfg(feature = "database")]
#[derive(Debug, Clone)]
pub enum InfrastructureError {
    /// Database connection or query failure
    Database { message: String, is_retryable: bool },
    /// Redis cache unavailable
    Cache { message: String },
    /// Missing or invalid configuration
    Configuration { message: String },
}

/// External service errors (payment providers, blockchain)
#[cfg(feature = "database")]
#[derive(Debug, Clone)]
pub enum ExternalError {
    /// Payment provider (Flutterwave, Paystack, M-Pesa) error
    PaymentProvider {
        provider: String,
        message: String,
        is_retryable: bool,
    },
    /// Stellar blockchain error
    Blockchain { message: String, is_retryable: bool },
    /// Rate limit exceeded
    RateLimit {
        service: String,
        retry_after: Option<u64>,
    },
    /// External service timeout
    Timeout { service: String, timeout_secs: u64 },
}

/// Input validation errors
#[cfg(feature = "database")]
#[derive(Debug, Clone)]
pub enum ValidationError {
    /// Invalid Stellar wallet address format
    InvalidWalletAddress { address: String, reason: String },
    /// Unsupported or invalid currency pair
    InvalidCurrency { currency: String, reason: String },
    /// Invalid amount (format or value)
    InvalidAmount { amount: String, reason: String },
    /// Required field missing
    MissingField { field: String },
    /// Field value out of acceptable range
    OutOfRange {
        field: String,
        min: Option<String>,
        max: Option<String>,
    },
}

/// Unified application error type
#[cfg(feature = "database")]
#[derive(Debug, Clone)]
pub struct AppError {
    pub kind: AppErrorKind,
    pub request_id: Option<String>,
    pub context: Option<String>,
}

#[cfg(feature = "database")]
#[derive(Debug, Clone)]
pub enum AppErrorKind {
    Domain(DomainError),
    Infrastructure(InfrastructureError),
    External(ExternalError),
    Validation(ValidationError),
}

#[cfg(feature = "database")]
impl AppError {
    pub fn new(kind: AppErrorKind) -> Self {
        Self {
            kind,
            request_id: None,
            context: None,
        }
    }

    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// Map error to HTTP status code
    pub fn status_code(&self) -> u16 {
        match &self.kind {
            AppErrorKind::Domain(err) => match err {
                DomainError::InsufficientLiquidity { .. } => 422,
                DomainError::AmountTooLow { .. } => 400,
                DomainError::InsufficientBalance { .. } => 422, // Unprocessable Entity
                DomainError::TrustlineNotFound { .. } => 422,
                DomainError::InvalidAmount { .. } => 400,
                DomainError::TransactionNotFound { .. } => 404,
                DomainError::WalletNotFound { .. } => 404,
                DomainError::RateExpired { .. } => 410, // Gone
                DomainError::DuplicateTransaction { .. } => 409, // Conflict
                DomainError::TrustlineCreationFailed { .. } => 422,
            },
            AppErrorKind::Infrastructure(err) => match err {
                InfrastructureError::Database { .. } => 500,
                InfrastructureError::Cache { .. } => 500,
                InfrastructureError::Configuration { .. } => 500,
            },
            AppErrorKind::External(err) => match err {
                ExternalError::PaymentProvider { .. } => 502, // Bad Gateway
                ExternalError::Blockchain { .. } => 502,
                ExternalError::RateLimit { .. } => 429, // Too Many Requests
                ExternalError::Timeout { .. } => 504,   // Gateway Timeout
            },
            AppErrorKind::Validation(err) => match err {
                ValidationError::InvalidWalletAddress { .. } => 400,
                ValidationError::InvalidCurrency { .. } => 400,
                ValidationError::InvalidAmount { .. } => 400,
                ValidationError::MissingField { .. } => 400,
                ValidationError::OutOfRange { .. } => 400,
            },
        }
    }

    /// Get error code for client handling
    pub fn error_code(&self) -> ErrorCode {
        match &self.kind {
            AppErrorKind::Domain(err) => match err {
                DomainError::InsufficientBalance { .. } => ErrorCode::InsufficientCngnBalance,
                DomainError::TrustlineNotFound { .. } => ErrorCode::TrustlineRequired,
                DomainError::InvalidAmount { .. } => ErrorCode::InvalidCngnAmount,
                DomainError::TransactionNotFound { .. } => ErrorCode::TransactionNotFound,
                DomainError::WalletNotFound { .. } => ErrorCode::WalletNotFound,
                DomainError::RateExpired { .. } => ErrorCode::RateExpired,
                DomainError::DuplicateTransaction { .. } => ErrorCode::DuplicateTransaction,
                DomainError::TrustlineCreationFailed { .. } => ErrorCode::TrustlineCreationFailed,
                DomainError::InsufficientLiquidity { .. } => ErrorCode::InsufficientLiquidity,
                DomainError::AmountTooLow { .. } => ErrorCode::AmountTooLow,
            },
            AppErrorKind::Infrastructure(err) => match err {
                InfrastructureError::Database { .. } => ErrorCode::DatabaseError,
                InfrastructureError::Cache { .. } => ErrorCode::CacheError,
                InfrastructureError::Configuration { .. } => ErrorCode::ConfigurationError,
            },
            AppErrorKind::External(err) => match err {
                ExternalError::PaymentProvider { .. } => ErrorCode::PaymentProviderError,
                ExternalError::Blockchain { .. } => ErrorCode::BlockchainError,
                ExternalError::RateLimit { .. } => ErrorCode::RateLimitError,
                ExternalError::Timeout { .. } => ErrorCode::ExternalServiceTimeout,
            },
            AppErrorKind::Validation(err) => match err {
                ValidationError::InvalidWalletAddress { .. } => ErrorCode::InvalidWallet,
                _ => ErrorCode::ValidationError,
            },
        }
    }

    /// Get user-friendly error message
    pub fn user_message(&self) -> String {
        match &self.kind {
            AppErrorKind::Domain(err) => match err {
                DomainError::InsufficientBalance {
                    available,
                    required,
                } => {
                    format!(
                        "Insufficient CNGN balance. Available: {}, Required: {}",
                        available, required
                    )
                }
                DomainError::TrustlineNotFound {
                    wallet_address,
                    asset,
                } => {
                    format!(
                        "Please add {} trustline to your wallet ({}...)",
                        asset,
                        &wallet_address[..6]
                    )
                }
                DomainError::InvalidAmount { amount, reason } => {
                    format!("Invalid amount '{}': {}", amount, reason)
                }
                DomainError::TransactionNotFound { transaction_id } => {
                    format!("Transaction '{}' not found", transaction_id)
                }
                DomainError::WalletNotFound { wallet_address } => {
                    format!("Wallet '{}...' not found", &wallet_address[..6])
                }
                DomainError::RateExpired { quote_id } => {
                    format!(
                        "Exchange rate quote '{}' has expired. Please request a new quote",
                        quote_id
                    )
                }
                DomainError::DuplicateTransaction { transaction_id } => {
                    format!("Transaction '{}' already exists", transaction_id)
                }
                DomainError::TrustlineCreationFailed {
                    wallet_address,
                    reason,
                } => {
                    format!(
                        "Failed to create trustline for wallet '{}...': {}",
                        &wallet_address[..6],
                        reason
                    )
                }
                DomainError::InsufficientLiquidity { .. } => {
                    "cNGN liquidity unavailable for this amount. Try a smaller amount or check back later.".to_string()
                }
                DomainError::AmountTooLow { .. } => {
                    "Minimum onramp amount is ₦1,000.".to_string()
                }
            },
            AppErrorKind::Infrastructure(_) => {
                "Service temporarily unavailable. Please try again later".to_string()
            }
            AppErrorKind::External(err) => {
                match err {
                    ExternalError::PaymentProvider {
                        provider,
                        is_retryable,
                        ..
                    } => {
                        if *is_retryable {
                            format!("Payment provider ({}) is temporarily unavailable. Please try again", provider)
                        } else {
                            "Payment processing failed. Please contact support".to_string()
                        }
                    }
                    ExternalError::Blockchain { is_retryable, .. } => {
                        if *is_retryable {
                            "Blockchain network is busy. Please try again in a moment".to_string()
                        } else {
                            "Blockchain operation failed. Please contact support".to_string()
                        }
                    }
                    ExternalError::RateLimit {
                        service,
                        retry_after,
                    } => {
                        if let Some(secs) = retry_after {
                            format!(
                                "Rate limit exceeded for {}. Please try again in {} seconds",
                                service, secs
                            )
                        } else {
                            format!(
                                "Rate limit exceeded for {}. Please try again later",
                                service
                            )
                        }
                    }
                    ExternalError::Timeout {
                        service,
                        timeout_secs,
                    } => {
                        format!(
                            "{} request timed out after {} seconds. Please try again",
                            service, timeout_secs
                        )
                    }
                }
            }
            AppErrorKind::Validation(err) => match err {
                ValidationError::InvalidWalletAddress { address, reason } => {
                    format!("Invalid wallet address '{}': {}", address, reason)
                }
                ValidationError::InvalidCurrency { currency, reason } => {
                    format!("Invalid currency '{}': {}", currency, reason)
                }
                ValidationError::InvalidAmount { amount, reason } => {
                    format!("Invalid amount '{}': {}", amount, reason)
                }
                ValidationError::MissingField { field } => {
                    format!("Required field '{}' is missing", field)
                }
                ValidationError::OutOfRange { field, min, max } => match (min, max) {
                    (Some(min), Some(max)) => {
                        format!("Field '{}' must be between {} and {}", field, min, max)
                    }
                    (Some(min), None) => {
                        format!("Field '{}' must be at least {}", field, min)
                    }
                    (None, Some(max)) => {
                        format!("Field '{}' must be at most {}", field, max)
                    }
                    (None, None) => {
                        format!("Field '{}' is out of acceptable range", field)
                    }
                },
            },
        }
    }

    /// Check if error is retryable
    pub fn is_retryable(&self) -> bool {
        match &self.kind {
            AppErrorKind::Domain(_) => false,
            AppErrorKind::Infrastructure(err) => match err {
                InfrastructureError::Database { is_retryable, .. } => *is_retryable,
                InfrastructureError::Cache { .. } => true,
                InfrastructureError::Configuration { .. } => false,
            },
            AppErrorKind::External(err) => match err {
                ExternalError::PaymentProvider { is_retryable, .. } => *is_retryable,
                ExternalError::Blockchain { is_retryable, .. } => *is_retryable,
                ExternalError::RateLimit { .. } => true,
                ExternalError::Timeout { .. } => true,
            },
            AppErrorKind::Validation(_) => false,
        }
    }
}

#[cfg(feature = "database")]
impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.user_message())
    }
}

#[cfg(feature = "database")]
impl std::error::Error for AppError {}

// Conversions from specific error types
// Note: From<DatabaseError> is implemented in database/error.rs to avoid circular dependency

#[cfg(feature = "database")]
impl From<StellarError> for AppError {
    fn from(err: StellarError) -> Self {
        use crate::chains::stellar::errors::StellarError as SE;

        let kind = match err {
            SE::AccountNotFound { address } => AppErrorKind::Domain(DomainError::WalletNotFound {
                wallet_address: address,
            }),
            SE::InvalidAddress { address } => {
                AppErrorKind::Validation(ValidationError::InvalidWalletAddress {
                    address,
                    reason: "Invalid Stellar address format".to_string(),
                })
            }
            SE::RateLimitError => AppErrorKind::External(ExternalError::RateLimit {
                service: "Stellar".to_string(),
                retry_after: Some(60),
            }),
            SE::TimeoutError { seconds } => AppErrorKind::External(ExternalError::Timeout {
                service: "Stellar".to_string(),
                timeout_secs: seconds,
            }),
            SE::NetworkError { message } | SE::UnexpectedError { message } => {
                AppErrorKind::External(ExternalError::Blockchain {
                    message,
                    is_retryable: true,
                })
            }
            SE::InsufficientXlm {
                available,
                required,
            } => AppErrorKind::Domain(DomainError::InsufficientBalance {
                available,
                required,
            }),
            SE::TrustlineAlreadyExists { address, asset } => {
                AppErrorKind::Domain(DomainError::DuplicateTransaction {
                    transaction_id: format!("trustline:{}:{}", address, asset),
                })
            }
            SE::TransactionFailed { message } | SE::SigningError { message } => {
                AppErrorKind::External(ExternalError::Blockchain {
                    message,
                    is_retryable: false,
                })
            }
            SE::ConfigError { message } => {
                AppErrorKind::Infrastructure(InfrastructureError::Configuration { message })
            }
            _ => AppErrorKind::External(ExternalError::Blockchain {
                message: err.to_string(),
                is_retryable: false,
            }),
        };

        AppError::new(kind)
    }
}

/// Result type for operations that can fail with AppError
#[cfg(feature = "database")]
pub type AppResult<T> = Result<T, AppError>;

#[cfg(all(test, feature = "database"))]
mod tests {
    use super::*;

    #[test]
    fn test_insufficient_balance_error() {
        let error = AppError::new(AppErrorKind::Domain(DomainError::InsufficientBalance {
            available: "50".to_string(),
            required: "100".to_string(),
        }));

        assert_eq!(error.status_code(), 422);
        assert_eq!(error.error_code(), ErrorCode::InsufficientCngnBalance);
        assert!(error.user_message().contains("Insufficient CNGN balance"));
        assert!(!error.is_retryable());
    }

    #[test]
    fn test_trustline_not_found_error() {
        let error = AppError::new(AppErrorKind::Domain(DomainError::TrustlineNotFound {
            wallet_address: "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string(),
            asset: "AFRI".to_string(),
        }));

        assert_eq!(error.status_code(), 422);
        assert_eq!(error.error_code(), ErrorCode::TrustlineRequired);
        assert!(error.user_message().contains("trustline"));
    }

    #[test]
    fn test_rate_limit_error() {
        let error = AppError::new(AppErrorKind::External(ExternalError::RateLimit {
            service: "Stellar".to_string(),
            retry_after: Some(60),
        }));

        assert_eq!(error.status_code(), 429);
        assert_eq!(error.error_code(), ErrorCode::RateLimitError);
        assert!(error.is_retryable());
    }

    #[test]
    fn test_validation_error() {
        let error = AppError::new(AppErrorKind::Validation(ValidationError::InvalidAmount {
            amount: "-100".to_string(),
            reason: "Amount cannot be negative".to_string(),
        }));

        assert_eq!(error.status_code(), 400);
        assert_eq!(error.error_code(), ErrorCode::ValidationError);
        assert!(!error.is_retryable());
    }
}
