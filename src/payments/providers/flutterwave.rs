use crate::payments::error::{PaymentError, PaymentResult};
use crate::payments::provider::PaymentProvider;
use crate::payments::types::{
    Money, PaymentMethod, PaymentRequest, PaymentResponse, PaymentState, ProviderName,
    StatusRequest, StatusResponse, WebhookEvent, WebhookVerificationResult, WithdrawalMethod,
    WithdrawalRequest, WithdrawalResponse,
};
use crate::payments::utils::{secure_eq, PaymentHttpClient};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::time::Duration;
use tracing::info;

#[derive(Debug, Clone)]
pub struct FlutterwaveConfig {
    pub secret_key: String,
    pub webhook_secret: Option<String>,
    pub base_url: String,
    pub timeout_secs: u64,
    pub max_retries: u32,
}

impl FlutterwaveConfig {
    pub fn from_env() -> PaymentResult<Self> {
        let secret_key =
            std::env::var("FLUTTERWAVE_SECRET_KEY").map_err(|_| PaymentError::ValidationError {
                message: "FLUTTERWAVE_SECRET_KEY environment variable is required".to_string(),
                field: Some("FLUTTERWAVE_SECRET_KEY".to_string()),
            })?;

        Ok(Self {
            secret_key,
            webhook_secret: std::env::var("FLUTTERWAVE_WEBHOOK_SECRET")
                .ok()
                .or_else(|| std::env::var("FLUTTERWAVE_WEBHOOK_HASH").ok()),
            base_url: std::env::var("FLUTTERWAVE_BASE_URL")
                .unwrap_or_else(|_| "https://api.flutterwave.com/v3".to_string()),
            timeout_secs: std::env::var("FLUTTERWAVE_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    std::env::var("PAYMENT_TIMEOUT_SECONDS")
                        .ok()
                        .and_then(|v| v.parse::<u64>().ok())
                })
                .unwrap_or(30),
            max_retries: std::env::var("FLUTTERWAVE_MAX_RETRIES")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(2),
        })
    }
}

pub struct FlutterwaveProvider {
    config: FlutterwaveConfig,
    http: PaymentHttpClient,
}

impl FlutterwaveProvider {
    pub fn new(config: FlutterwaveConfig) -> PaymentResult<Self> {
        let http =
            PaymentHttpClient::new(Duration::from_secs(config.timeout_secs), config.max_retries)?;
        Ok(Self { config, http })
    }

    pub fn from_env() -> PaymentResult<Self> {
        Self::new(FlutterwaveConfig::from_env()?)
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.config.base_url, path)
    }

    fn ensure_status_ref(request: &StatusRequest) -> PaymentResult<String> {
        request
            .provider_reference
            .clone()
            .or_else(|| request.transaction_reference.clone())
            .filter(|v| !v.trim().is_empty())
            .ok_or(PaymentError::ValidationError {
                message: "provider_reference or transaction_reference is required".to_string(),
                field: Some("reference".to_string()),
            })
    }

    fn map_message_error(message: String) -> PaymentError {
        let lowered = message.to_lowercase();
        if lowered.contains("insufficient") || lowered.contains("low balance") {
            return PaymentError::InsufficientFundsError { message };
        }
        if lowered.contains("declined")
            || lowered.contains("do not honor")
            || lowered.contains("expired card")
        {
            return PaymentError::PaymentDeclinedError {
                message,
                provider_code: None,
            };
        }
        if lowered.contains("too many requests") || lowered.contains("rate limit") {
            return PaymentError::RateLimitError {
                message,
                retry_after_seconds: None,
            };
        }
        if lowered.contains("invalid")
            || lowered.contains("missing")
            || lowered.contains("not found")
            || lowered.contains("unsupported")
        {
            return PaymentError::ValidationError {
                message,
                field: None,
            };
        }
        PaymentError::ProviderError {
            provider: "flutterwave".to_string(),
            message,
            provider_code: None,
            retryable: false,
        }
    }
}

#[async_trait]
impl PaymentProvider for FlutterwaveProvider {
    async fn initiate_payment(&self, request: PaymentRequest) -> PaymentResult<PaymentResponse> {
        request.amount.validate_positive("amount")?;
        if request.transaction_reference.trim().is_empty() {
            return Err(PaymentError::ValidationError {
                message: "transaction_reference is required".to_string(),
                field: Some("transaction_reference".to_string()),
            });
        }
        if request
            .customer
            .email
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return Err(PaymentError::ValidationError {
                message: "customer.email is required for flutterwave initialization".to_string(),
                field: Some("customer.email".to_string()),
            });
        }

        let payment_options = match request.payment_method {
            PaymentMethod::Card => "card",
            PaymentMethod::BankTransfer => "banktransfer",
            PaymentMethod::MobileMoney => "mobilemoney",
            PaymentMethod::Ussd => "ussd",
            PaymentMethod::Wallet | PaymentMethod::Other => "card,banktransfer,ussd",
        };

        let payload = serde_json::json!({
            "tx_ref": request.transaction_reference,
            "amount": request.amount.amount,
            "currency": request.amount.currency,
            "redirect_url": request.callback_url,
            "payment_options": payment_options,
            "customer": {
                "email": request.customer.email,
                "phonenumber": request.customer.phone,
            },
            "meta": request.metadata,
            "customizations": {
                "title": "Aframp Payment",
            }
        });

        let raw: FlutterwaveEnvelope = self
            .http
            .request_json(
                reqwest::Method::POST,
                &self.endpoint("/payments"),
                Some(&self.config.secret_key),
                Some(&payload),
                &[("Content-Type", "application/json")],
            )
            .await
            .map_err(|e| match e {
                PaymentError::ProviderError { message, .. } => Self::map_message_error(message),
                other => other,
            })?;

        if raw.status.to_lowercase() != "success" {
            return Err(Self::map_message_error(raw.message));
        }

        let payment_link = raw
            .data
            .as_ref()
            .and_then(|v| v.get("link"))
            .and_then(|v| v.as_str())
            .or_else(|| {
                raw.data
                    .as_ref()
                    .and_then(|v| v.get("checkout_url"))
                    .and_then(|v| v.as_str())
            })
            .map(|v| v.to_string())
            .ok_or(PaymentError::ProviderError {
                provider: "flutterwave".to_string(),
                message: "missing payment link in flutterwave response".to_string(),
                provider_code: None,
                retryable: false,
            })?;

        info!(
            tx_ref = %request.transaction_reference,
            "flutterwave payment initiated"
        );

        Ok(PaymentResponse {
            status: PaymentState::Pending,
            transaction_reference: request.transaction_reference.clone(),
            provider_reference: Some(request.transaction_reference),
            payment_url: Some(payment_link),
            amount_charged: Some(request.amount),
            fees_charged: None,
            provider_data: raw.data,
        })
    }

    async fn verify_payment(&self, request: StatusRequest) -> PaymentResult<StatusResponse> {
        let reference = Self::ensure_status_ref(&request)?;
        let url = format!(
            "{}?tx_ref={}",
            self.endpoint("/transactions/verify_by_reference"),
            reference
        );
        let raw: FlutterwaveEnvelope = self
            .http
            .request_json(
                reqwest::Method::GET,
                &url,
                Some(&self.config.secret_key),
                None,
                &[],
            )
            .await
            .map_err(|e| match e {
                PaymentError::ProviderError { message, .. } => Self::map_message_error(message),
                other => other,
            })?;

        if raw.status.to_lowercase() != "success" {
            return Err(Self::map_message_error(raw.message));
        }

        let data = raw.data.unwrap_or_else(|| serde_json::json!({}));
        let tx_status = data
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_lowercase();
        let status = match tx_status.as_str() {
            "successful" | "success" | "completed" => PaymentState::Success,
            "pending" => PaymentState::Pending,
            "failed" | "cancelled" => PaymentState::Failed,
            _ => PaymentState::Unknown,
        };

        let amount = data
            .get("amount")
            .and_then(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| v.as_f64().map(|n| n.to_string()))
            })
            .map(|amount| Money {
                amount,
                currency: data
                    .get("currency")
                    .and_then(|v| v.as_str())
                    .unwrap_or("NGN")
                    .to_string(),
            });

        let method = match data
            .get("payment_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase()
            .as_str()
        {
            "card" => Some(PaymentMethod::Card),
            "banktransfer" | "bank_transfer" => Some(PaymentMethod::BankTransfer),
            "mobilemoney" | "mobile_money" => Some(PaymentMethod::MobileMoney),
            "ussd" => Some(PaymentMethod::Ussd),
            _ => Some(PaymentMethod::Other),
        };

        Ok(StatusResponse {
            status,
            transaction_reference: data
                .get("tx_ref")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or(request.transaction_reference),
            provider_reference: data
                .get("flw_ref")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or(Some(reference)),
            amount,
            payment_method: method,
            timestamp: data
                .get("created_at")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            failure_reason: data
                .get("processor_response")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            provider_data: Some(data),
        })
    }

    async fn process_withdrawal(
        &self,
        request: WithdrawalRequest,
    ) -> PaymentResult<WithdrawalResponse> {
        request.amount.validate_positive("amount")?;
        if !matches!(request.withdrawal_method, WithdrawalMethod::BankTransfer) {
            return Err(PaymentError::ValidationError {
                message: "flutterwave currently supports bank transfer withdrawals only"
                    .to_string(),
                field: Some("withdrawal_method".to_string()),
            });
        }

        let account_number =
            request
                .recipient
                .account_number
                .clone()
                .ok_or(PaymentError::ValidationError {
                    message: "recipient.account_number is required".to_string(),
                    field: Some("recipient.account_number".to_string()),
                })?;
        let bank_code =
            request
                .recipient
                .bank_code
                .clone()
                .ok_or(PaymentError::ValidationError {
                    message: "recipient.bank_code is required".to_string(),
                    field: Some("recipient.bank_code".to_string()),
                })?;

        let payload = serde_json::json!({
            "account_bank": bank_code,
            "account_number": account_number,
            "amount": request.amount.amount,
            "currency": request.amount.currency,
            "reference": request.transaction_reference,
            "narration": request.reason.unwrap_or_else(|| "Aframp payout".to_string()),
            "debit_currency": "NGN",
            "meta": request.metadata,
        });

        let raw: FlutterwaveEnvelope = self
            .http
            .request_json(
                reqwest::Method::POST,
                &self.endpoint("/transfers"),
                Some(&self.config.secret_key),
                Some(&payload),
                &[("Content-Type", "application/json")],
            )
            .await
            .map_err(|e| match e {
                PaymentError::ProviderError { message, .. } => Self::map_message_error(message),
                other => other,
            })?;

        if raw.status.to_lowercase() != "success" {
            return Err(Self::map_message_error(raw.message));
        }

        let data = raw.data.unwrap_or_else(|| serde_json::json!({}));
        let transfer_status = data
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_lowercase();
        let status = match transfer_status.as_str() {
            "successful" | "success" | "completed" => PaymentState::Success,
            "new" | "pending" | "processing" => PaymentState::Processing,
            "failed" | "cancelled" => PaymentState::Failed,
            _ => PaymentState::Unknown,
        };

        let provider_reference = data
            .get("reference")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                data.get("id")
                    .and_then(|v| v.as_i64())
                    .map(|id| id.to_string())
            });

        Ok(WithdrawalResponse {
            status,
            transaction_reference: request.transaction_reference,
            provider_reference,
            amount_debited: Some(request.amount),
            fees_charged: None,
            estimated_completion_seconds: Some(120),
            provider_data: Some(data),
        })
    }

    async fn get_payment_status(&self, request: StatusRequest) -> PaymentResult<StatusResponse> {
        self.verify_payment(request).await
    }

    fn name(&self) -> ProviderName {
        ProviderName::Flutterwave
    }

    fn supported_currencies(&self) -> &'static [&'static str] {
        &["NGN", "GHS", "KES", "ZAR", "USD"]
    }

    fn supported_countries(&self) -> &'static [&'static str] {
        &["NG", "GH", "KE", "ZA", "US"]
    }

    fn verify_webhook(
        &self,
        _payload: &[u8],
        signature: &str,
    ) -> PaymentResult<WebhookVerificationResult> {
        let expected = self.config.webhook_secret.as_deref().ok_or(
            PaymentError::WebhookVerificationError {
                message: "FLUTTERWAVE_WEBHOOK_SECRET is not configured".to_string(),
            },
        )?;
        let valid = secure_eq(expected.trim().as_bytes(), signature.trim().as_bytes());
        Ok(WebhookVerificationResult {
            valid,
            reason: if valid {
                None
            } else {
                Some("invalid flutterwave webhook hash".to_string())
            },
        })
    }

    fn parse_webhook_event(&self, payload: &[u8]) -> PaymentResult<WebhookEvent> {
        let parsed: JsonValue = serde_json::from_slice(payload).map_err(|e| {
            PaymentError::WebhookVerificationError {
                message: format!("invalid webhook JSON payload: {}", e),
            }
        })?;

        let event_type = parsed
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let data = parsed
            .get("data")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let status =
            data.get("status")
                .and_then(|v| v.as_str())
                .map(|s| match s.to_lowercase().as_str() {
                    "successful" | "success" | "completed" => PaymentState::Success,
                    "pending" | "new" | "processing" => PaymentState::Pending,
                    "failed" | "cancelled" => PaymentState::Failed,
                    _ => PaymentState::Unknown,
                });

        let provider_reference = data
            .get("flw_ref")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                data.get("id")
                    .and_then(|v| v.as_i64())
                    .map(|id| id.to_string())
            })
            .or_else(|| {
                data.get("reference")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            });

        Ok(WebhookEvent {
            provider: ProviderName::Flutterwave,
            event_type,
            transaction_reference: data
                .get("tx_ref")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    data.get("reference")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                }),
            provider_reference,
            status,
            payload: parsed,
            received_at: chrono::Utc::now().to_rfc3339(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct FlutterwaveEnvelope {
    status: String,
    message: String,
    #[serde(default)]
    data: Option<JsonValue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> FlutterwaveProvider {
        FlutterwaveProvider::new(FlutterwaveConfig {
            secret_key: "FLWSECK_TEST_demo".to_string(),
            webhook_secret: Some("hash_123".to_string()),
            base_url: "https://api.flutterwave.com/v3".to_string(),
            timeout_secs: 5,
            max_retries: 1,
        })
        .expect("provider init should succeed")
    }

    #[test]
    fn webhook_signature_validation_works() {
        let provider = provider();
        let valid = provider
            .verify_webhook(br#"{"event":"charge.completed"}"#, "hash_123")
            .expect("verification should not error");
        assert!(valid.valid);

        let invalid = provider
            .verify_webhook(br#"{"event":"charge.completed"}"#, "wrong")
            .expect("verification should not error");
        assert!(!invalid.valid);
    }

    #[test]
    fn parse_webhook_event_maps_fields() {
        let provider = provider();
        let payload = br#"{
            "event":"charge.completed",
            "data":{
                "status":"successful",
                "tx_ref":"tx_ref_1",
                "flw_ref":"flw_1"
            }
        }"#;
        let event = provider
            .parse_webhook_event(payload)
            .expect("webhook parse should succeed");
        assert_eq!(event.event_type, "charge.completed");
        assert_eq!(event.transaction_reference.as_deref(), Some("tx_ref_1"));
        assert_eq!(event.provider_reference.as_deref(), Some("flw_1"));
        assert!(matches!(event.status, Some(PaymentState::Success)));
    }
}
