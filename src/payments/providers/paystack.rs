//! Paystack payment provider implementation
//!
//! This module provides integration with Paystack's payment API for processing
//! payments in Nigeria (NGN), Ghana (GHS), and South Africa (ZAR).

use crate::error::{AppError, AppErrorKind, ExternalError};
use crate::payments::traits::PaymentProvider;
use crate::payments::types::{
    PaymentRequest, PaymentResponse, PaymentStatus, WithdrawalRequest, WithdrawalResponse,
    WithdrawalStatus,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{error, info, warn};

/// Paystack payment provider configuration
#[derive(Debug, Clone)]
pub struct PaystackConfig {
    /// Paystack API secret key
    pub secret_key: String,
    /// Paystack API base URL (defaults to https://api.paystack.co)
    pub base_url: String,
    /// Request timeout in seconds
    pub timeout_secs: u64,
    /// Maximum number of retries for failed requests
    pub max_retries: u32,
}

impl Default for PaystackConfig {
    fn default() -> Self {
        Self {
            secret_key: String::new(),
            base_url: "https://api.paystack.co".to_string(),
            timeout_secs: 30,
            max_retries: 3,
        }
    }
}

impl PaystackConfig {
    /// Create config from environment variables
    pub fn from_env() -> Result<Self, AppError> {
        let secret_key = std::env::var("PAYSTACK_SECRET_KEY")
            .map_err(|_| {
                AppError::new(AppErrorKind::Infrastructure(
                    crate::error::InfrastructureError::Configuration {
                        message: "PAYSTACK_SECRET_KEY environment variable is required"
                            .to_string(),
                    },
                ))
            })?;

        let base_url = std::env::var("PAYSTACK_BASE_URL")
            .unwrap_or_else(|_| "https://api.paystack.co".to_string());

        let timeout_secs = std::env::var("PAYSTACK_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);

        let max_retries = std::env::var("PAYSTACK_MAX_RETRIES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3);

        Ok(Self {
            secret_key,
            base_url,
            timeout_secs,
            max_retries,
        })
    }
}

/// Paystack payment provider
pub struct PaystackProvider {
    config: PaystackConfig,
    client: Client,
}

impl PaystackProvider {
    /// Create a new Paystack provider instance
    pub fn new(config: PaystackConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("Failed to create HTTP client");

        Self { config, client }
    }

    /// Create provider from environment variables
    pub fn from_env() -> Result<Self, AppError> {
        let config = PaystackConfig::from_env()?;
        Ok(Self::new(config))
    }

    /// Make an authenticated request to Paystack API
    async fn make_request<T>(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, AppError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let url = format!("{}{}", self.config.base_url, endpoint);
        let mut request = self
            .client
            .request(method, &url)
            .header("Authorization", format!("Bearer {}", self.config.secret_key))
            .header("Content-Type", "application/json");

        if let Some(body) = body {
            request = request.json(body);
        }

        let mut last_error = None;
        for attempt in 0..=self.config.max_retries {
            match request.try_clone() {
                Some(req) => {
                    match req.send().await {
                        Ok(response) => {
                            let status = response.status();
                            let response_text = response.text().await.unwrap_or_default();

                            if status.is_success() {
                                match serde_json::from_str::<PaystackResponse<T>>(&response_text) {
                                    Ok(paystack_resp) => {
                                        if paystack_resp.status {
                                            return Ok(paystack_resp.data);
                                        } else {
                                            let error_msg = paystack_resp.message;
                                            error!(
                                                "Paystack API error: {}",
                                                error_msg
                                            );
                                            return Err(AppError::new(
                                                AppErrorKind::External(ExternalError::PaymentProvider {
                                                    provider: "Paystack".to_string(),
                                                    message: error_msg,
                                                    is_retryable: false,
                                                }),
                                            ));
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to parse Paystack response: {}", e);
                                        return Err(AppError::new(
                                            AppErrorKind::External(ExternalError::PaymentProvider {
                                                provider: "Paystack".to_string(),
                                                message: format!("Invalid response format: {}", e),
                                                is_retryable: false,
                                            }),
                                        ));
                                    }
                                }
                            } else if status == 429 {
                                // Rate limit - retry with backoff
                                if attempt < self.config.max_retries {
                                    let backoff = 2_u64.pow(attempt);
                                    warn!(
                                        "Rate limited, retrying after {} seconds (attempt {})",
                                        backoff,
                                        attempt + 1
                                    );
                                    tokio::time::sleep(Duration::from_secs(backoff)).await;
                                    continue;
                                }
                                return Err(AppError::new(AppErrorKind::External(
                                    ExternalError::RateLimit {
                                        service: "Paystack".to_string(),
                                        retry_after: Some(60),
                                    },
                                )));
                            } else if status.is_server_error() && attempt < self.config.max_retries {
                                // Server error - retry
                                let backoff = 2_u64.pow(attempt);
                                warn!(
                                    "Server error {}, retrying after {} seconds (attempt {})",
                                    status,
                                    backoff,
                                    attempt + 1
                                );
                                tokio::time::sleep(Duration::from_secs(backoff)).await;
                                continue;
                            } else {
                                let error_msg = format!("HTTP {}: {}", status, response_text);
                                error!("Paystack API error: {}", error_msg);
                                return Err(AppError::new(AppErrorKind::External(
                                    ExternalError::PaymentProvider {
                                        provider: "Paystack".to_string(),
                                        message: error_msg,
                                        is_retryable: status.is_server_error(),
                                    },
                                )));
                            }
                        }
                        Err(e) => {
                            last_error = Some(e);
                            if attempt < self.config.max_retries {
                                let backoff = 2_u64.pow(attempt);
                                warn!(
                                    "Request error, retrying after {} seconds (attempt {}): {}",
                                    backoff,
                                    attempt + 1,
                                    last_error.as_ref().unwrap()
                                );
                                tokio::time::sleep(Duration::from_secs(backoff)).await;
                                continue;
                            }
                        }
                    }
                }
                None => {
                    return Err(AppError::new(AppErrorKind::External(
                        ExternalError::PaymentProvider {
                            provider: "Paystack".to_string(),
                            message: "Failed to clone request".to_string(),
                            is_retryable: false,
                        },
                    )));
                }
            }
        }

        Err(AppError::new(AppErrorKind::External(
            ExternalError::PaymentProvider {
                provider: "Paystack".to_string(),
                message: format!(
                    "Request failed after {} retries: {}",
                    self.config.max_retries,
                    last_error
                        .as_ref()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "Unknown error".to_string())
                ),
                is_retryable: true,
            },
        )))
    }
}

#[async_trait]
impl PaymentProvider for PaystackProvider {
    async fn initiate_payment(&self, request: PaymentRequest) -> crate::error::AppResult<PaymentResponse> {
        info!(
            "Initiating Paystack payment: {} {} {}",
            request.amount, request.currency, request.reference
        );

        let mut payload = serde_json::json!({
            "email": request.email,
            "amount": request.amount,
            "currency": request.currency,
            "reference": request.reference,
        });

        if let Some(callback_url) = request.callback_url {
            payload["callback_url"] = serde_json::Value::String(callback_url);
        }

        if let Some(channels) = request.channels {
            payload["channels"] = serde_json::Value::Array(
                channels
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            );
        }

        if let Some(metadata) = request.metadata {
            payload["metadata"] = metadata;
        }

        let response: PaystackInitializeResponse = self
            .make_request(reqwest::Method::POST, "/transaction/initialize", Some(&payload))
            .await?;

        info!(
            "Paystack payment initiated successfully: reference={}",
            response.reference
        );

        Ok(PaymentResponse {
            authorization_url: Some(response.authorization_url),
            access_code: Some(response.access_code),
            reference: response.reference,
            provider_data: Some(serde_json::json!({
                "access_code": response.access_code,
            })),
        })
    }

    async fn verify_payment(&self, reference: &str) -> crate::error::AppResult<PaymentStatus> {
        info!("Verifying Paystack payment: reference={}", reference);

        let response: PaystackVerifyResponse = self
            .make_request(
                reqwest::Method::GET,
                &format!("/transaction/verify/{}", reference),
                None,
            )
            .await?;

        info!(
            "Paystack payment verified: reference={}, status={}",
            reference, response.status
        );

        let status = match response.status.as_str() {
            "success" => PaymentStatus::Success {
                amount: response.amount.to_string(),
                currency: response.currency,
                paid_at: response.paid_at,
                channel: Some(response.channel),
            },
            "pending" => PaymentStatus::Pending,
            "failed" => PaymentStatus::Failed {
                reason: response.gateway_response,
            },
            "reversed" => PaymentStatus::Reversed,
            _ => PaymentStatus::Unknown,
        };

        Ok(status)
    }

    async fn process_withdrawal(
        &self,
        request: WithdrawalRequest,
    ) -> crate::error::AppResult<WithdrawalResponse> {
        info!(
            "Processing Paystack withdrawal: {} {} {}",
            request.amount, request.currency, request.reference
        );

        // Step 1: Create transfer recipient
        let recipient_payload = serde_json::json!({
            "type": "nuban",
            "name": request.recipient_name,
            "account_number": request.account_number,
            "bank_code": request.bank_code,
            "currency": request.currency,
        });

        let recipient: PaystackRecipientResponse = self
            .make_request(
                reqwest::Method::POST,
                "/transferrecipient",
                Some(&recipient_payload),
            )
            .await?;

        info!(
            "Paystack recipient created: recipient_code={}",
            recipient.recipient_code
        );

        // Step 2: Initiate transfer
        let mut transfer_payload = serde_json::json!({
            "source": "balance",
            "amount": request.amount,
            "recipient": recipient.recipient_code,
            "reference": request.reference,
        });

        if let Some(reason) = request.reason {
            transfer_payload["reason"] = serde_json::Value::String(reason);
        }

        if let Some(metadata) = request.metadata {
            transfer_payload["metadata"] = metadata;
        }

        let transfer: PaystackTransferResponse = self
            .make_request(reqwest::Method::POST, "/transfer", Some(&transfer_payload))
            .await?;

        info!(
            "Paystack withdrawal processed: transfer_code={}, status={}",
            transfer.transfer_code, transfer.status
        );

        let withdrawal_status = match transfer.status.as_str() {
            "success" => WithdrawalStatus::Success,
            "pending" => WithdrawalStatus::Pending,
            "failed" => WithdrawalStatus::Failed {
                reason: transfer.failure_reason,
            },
            "reversed" => WithdrawalStatus::Reversed,
            _ => WithdrawalStatus::Pending,
        };

        Ok(WithdrawalResponse {
            transfer_reference: transfer.reference,
            status: withdrawal_status,
            provider_data: Some(serde_json::json!({
                "transfer_code": transfer.transfer_code,
                "recipient_code": recipient.recipient_code,
            })),
        })
    }

    fn validate_webhook_signature(&self, payload: &[u8], signature: &str) -> bool {
        use hmac::{Hmac, Mac};
        use sha2::Sha512;

        type HmacSha512 = Hmac<Sha512>;

        let mut mac = HmacSha512::new_from_slice(self.config.secret_key.as_bytes())
            .expect("HMAC can take key of any size");

        mac.update(payload);
        let computed_signature = hex::encode(mac.finalize().into_bytes());

        // Paystack sends signature as hex string
        let provided_signature = signature.trim();

        // Constant-time comparison to prevent timing attacks
        // Using bytes comparison for constant-time operation
        if computed_signature.len() != provided_signature.len() {
            return false;
        }

        computed_signature
            .as_bytes()
            .iter()
            .zip(provided_signature.as_bytes().iter())
            .fold(0, |acc, (a, b)| acc | (a ^ b))
            == 0
    }
}

// Paystack API response wrapper
#[derive(Debug, Deserialize)]
struct PaystackResponse<T> {
    status: bool,
    message: String,
    data: T,
}

// Initialize transaction response
#[derive(Debug, Deserialize)]
struct PaystackInitializeResponse {
    authorization_url: String,
    access_code: String,
    reference: String,
}

// Verify transaction response
#[derive(Debug, Deserialize)]
struct PaystackVerifyResponse {
    amount: u64,
    currency: String,
    status: String,
    channel: String,
    #[serde(default)]
    paid_at: Option<String>,
    #[serde(default)]
    gateway_response: Option<String>,
}

// Transfer recipient response
#[derive(Debug, Deserialize)]
struct PaystackRecipientResponse {
    recipient_code: String,
}

// Transfer response
#[derive(Debug, Deserialize)]
struct PaystackTransferResponse {
    transfer_code: String,
    reference: String,
    status: String,
    #[serde(default)]
    failure_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_provider() -> PaystackProvider {
        let config = PaystackConfig {
            secret_key: "sk_test_test_key".to_string(),
            base_url: "https://api.paystack.co".to_string(),
            timeout_secs: 30,
            max_retries: 3,
        };
        PaystackProvider::new(config)
    }

    #[test]
    fn test_webhook_signature_validation_invalid() {
        let provider = create_test_provider();
        let payload = b"test payload";
        let signature = "invalid_signature";
        let result = provider.validate_webhook_signature(payload, signature);
        assert!(!result, "Invalid signature should return false");
    }

    #[test]
    fn test_paystack_config_default() {
        let config = PaystackConfig::default();
        assert_eq!(config.base_url, "https://api.paystack.co");
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_paystack_config_from_env_missing_key() {
        std::env::remove_var("PAYSTACK_SECRET_KEY");
        
        let config = PaystackConfig::from_env();
        assert!(config.is_err(), "Config should fail without secret key");
    }
}
