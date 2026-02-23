use thiserror::Error;

pub type PaymentResult<T> = Result<T, PaymentError>;

#[derive(Debug, Clone, Error)]
pub enum PaymentError {
    #[error("Validation error: {message}")]
    ValidationError {
        message: String,
        field: Option<String>,
    },

    #[error("Insufficient funds: {message}")]
    InsufficientFundsError { message: String },

    #[error("Payment declined: {message}")]
    PaymentDeclinedError {
        message: String,
        provider_code: Option<String>,
    },

    #[error("Network error: {message}")]
    NetworkError { message: String },

    #[error("Rate limit exceeded: {message}")]
    RateLimitError {
        message: String,
        retry_after_seconds: Option<u64>,
    },

    #[error("Webhook verification failed: {message}")]
    WebhookVerificationError { message: String },

    #[error("Provider error: provider={provider}, message={message}")]
    ProviderError {
        provider: String,
        message: String,
        provider_code: Option<String>,
        retryable: bool,
    },
}

impl PaymentError {
    pub fn is_retryable(&self) -> bool {
        match self {
            PaymentError::ValidationError { .. } => false,
            PaymentError::InsufficientFundsError { .. } => false,
            PaymentError::PaymentDeclinedError { .. } => false,
            PaymentError::NetworkError { .. } => true,
            PaymentError::RateLimitError { .. } => true,
            PaymentError::WebhookVerificationError { .. } => false,
            PaymentError::ProviderError { retryable, .. } => *retryable,
        }
    }

    pub fn http_status_code(&self) -> u16 {
        match self {
            PaymentError::ValidationError { .. } => 400,
            PaymentError::InsufficientFundsError { .. } => 402,
            PaymentError::PaymentDeclinedError { .. } => 402,
            PaymentError::NetworkError { .. } => 503,
            PaymentError::RateLimitError { .. } => 429,
            PaymentError::WebhookVerificationError { .. } => 401,
            PaymentError::ProviderError { .. } => 502,
        }
    }

    pub fn user_message(&self) -> String {
        match self {
            PaymentError::ValidationError { message, .. } => message.clone(),
            PaymentError::InsufficientFundsError { .. } => {
                "Insufficient funds to complete payment".to_string()
            }
            PaymentError::PaymentDeclinedError { .. } => {
                "Payment was declined by the provider".to_string()
            }
            PaymentError::NetworkError { .. } => {
                "Payment provider is temporarily unavailable".to_string()
            }
            PaymentError::RateLimitError { .. } => {
                "Too many requests to payment provider. Please retry shortly".to_string()
            }
            PaymentError::WebhookVerificationError { .. } => {
                "Invalid webhook signature".to_string()
            }
            PaymentError::ProviderError { .. } => "Payment provider returned an error".to_string(),
        }
    }
}

impl From<PaymentError> for crate::error::AppError {
    fn from(err: PaymentError) -> Self {
        use crate::error::{AppError, AppErrorKind, ExternalError};

        AppError::new(AppErrorKind::External(ExternalError::PaymentProvider {
            provider: "payments".to_string(),
            message: err.to_string(),
            is_retryable: err.is_retryable(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_http_status_mapping_is_correct() {
        assert_eq!(
            PaymentError::ValidationError {
                message: "bad".to_string(),
                field: None
            }
            .http_status_code(),
            400
        );
        assert_eq!(
            PaymentError::RateLimitError {
                message: "limited".to_string(),
                retry_after_seconds: Some(30)
            }
            .http_status_code(),
            429
        );
    }

    #[test]
    fn retryable_flags_are_set() {
        assert!(PaymentError::NetworkError {
            message: "timeout".to_string()
        }
        .is_retryable());
        assert!(!PaymentError::PaymentDeclinedError {
            message: "declined".to_string(),
            provider_code: None
        }
        .is_retryable());
    }
}
