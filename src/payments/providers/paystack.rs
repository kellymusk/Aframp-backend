use crate::payments::error::{PaymentError, PaymentResult};
use crate::payments::provider::PaymentProvider;
use crate::payments::types::{
    Money, PaymentMethod, PaymentRequest, PaymentResponse, PaymentState, ProviderName,
    StatusRequest, StatusResponse, WebhookEvent, WebhookVerificationResult, WithdrawalMethod,
    WithdrawalRequest, WithdrawalResponse,
};
use crate::payments::utils::{verify_hmac_sha512_hex, PaymentHttpClient};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::time::Duration;
use tracing::info;

#[derive(Debug, Clone)]
pub struct PaystackConfig {
    pub public_key: Option<String>,
    pub secret_key: String,
    pub webhook_secret: Option<String>,
    pub base_url: String,
    pub timeout_secs: u64,
    pub max_retries: u32,
}

impl Default for PaystackConfig {
    fn default() -> Self {
        Self {
            public_key: None,
            secret_key: String::new(),
            webhook_secret: None,
            base_url: "https://api.paystack.co".to_string(),
            timeout_secs: 30,
            max_retries: 3,
        }
    }
}

impl PaystackConfig {
    pub fn from_env() -> PaymentResult<Self> {
        let secret_key =
            std::env::var("PAYSTACK_SECRET_KEY").map_err(|_| PaymentError::ValidationError {
                message: "PAYSTACK_SECRET_KEY environment variable is required".to_string(),
                field: Some("PAYSTACK_SECRET_KEY".to_string()),
            })?;

        Ok(Self {
            public_key: std::env::var("PAYSTACK_PUBLIC_KEY").ok(),
            webhook_secret: std::env::var("PAYSTACK_WEBHOOK_SECRET").ok(),
            base_url: std::env::var("PAYSTACK_BASE_URL")
                .unwrap_or_else(|_| "https://api.paystack.co".to_string()),
            timeout_secs: std::env::var("PAYSTACK_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(30),
            max_retries: std::env::var("PAYSTACK_MAX_RETRIES")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(3),
            secret_key,
        })
    }
}

pub struct PaystackProvider {
    config: PaystackConfig,
    http: PaymentHttpClient,
}

impl PaystackProvider {
    pub fn new(config: PaystackConfig) -> PaymentResult<Self> {
        let http =
            PaymentHttpClient::new(Duration::from_secs(config.timeout_secs), config.max_retries)?;
        Ok(Self { config, http })
    }

    pub fn from_env() -> PaymentResult<Self> {
        Self::new(PaystackConfig::from_env()?)
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
}

#[async_trait]
impl PaymentProvider for PaystackProvider {
    async fn initiate_payment(&self, request: PaymentRequest) -> PaymentResult<PaymentResponse> {
        request.amount.validate_positive("amount")?;
        if request
            .customer
            .email
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return Err(PaymentError::ValidationError {
                message: "customer.email is required for paystack initialization".to_string(),
                field: Some("customer.email".to_string()),
            });
        }

        let payload = serde_json::json!({
            "email": request.customer.email,
            "amount": request.amount.amount,
            "currency": request.amount.currency,
            "reference": request.transaction_reference,
            "callback_url": request.callback_url,
            "metadata": request.metadata,
        });

        let raw: PaystackEnvelope<PaystackInitializeData> = self
            .http
            .request_json(
                reqwest::Method::POST,
                &self.endpoint("/transaction/initialize"),
                Some(&self.config.secret_key),
                Some(&payload),
                &[("Content-Type", "application/json")],
            )
            .await?;

        if !raw.status {
            return Err(PaymentError::ProviderError {
                provider: "paystack".to_string(),
                message: raw.message,
                provider_code: None,
                retryable: false,
            });
        }
        let data = raw.data;
        info!(reference = %data.reference, "paystack payment initiated");

        Ok(PaymentResponse {
            status: PaymentState::Pending,
            transaction_reference: request.transaction_reference,
            provider_reference: Some(data.reference.clone()),
            payment_url: Some(data.authorization_url),
            amount_charged: Some(request.amount),
            fees_charged: None,
            provider_data: Some(serde_json::json!({
                "access_code": data.access_code,
                "provider_reference": data.reference
            })),
        })
    }

    async fn verify_payment(&self, request: StatusRequest) -> PaymentResult<StatusResponse> {
        let reference = Self::ensure_status_ref(&request)?;
        let raw: PaystackEnvelope<PaystackVerifyData> = self
            .http
            .request_json(
                reqwest::Method::GET,
                &self.endpoint(&format!("/transaction/verify/{}", reference)),
                Some(&self.config.secret_key),
                None,
                &[],
            )
            .await?;
        if !raw.status {
            return Err(PaymentError::ProviderError {
                provider: "paystack".to_string(),
                message: raw.message,
                provider_code: None,
                retryable: false,
            });
        }

        let status = match raw.data.status.as_str() {
            "success" => PaymentState::Success,
            "pending" => PaymentState::Pending,
            "failed" => PaymentState::Failed,
            "abandoned" => PaymentState::Cancelled,
            "reversed" => PaymentState::Reversed,
            _ => PaymentState::Unknown,
        };

        Ok(StatusResponse {
            status,
            transaction_reference: request.transaction_reference,
            provider_reference: Some(reference),
            amount: Some(Money {
                amount: raw.data.amount.to_string(),
                currency: raw.data.currency,
            }),
            payment_method: Some(match raw.data.channel.as_str() {
                "card" => PaymentMethod::Card,
                "bank" | "bank_transfer" => PaymentMethod::BankTransfer,
                "mobile_money" => PaymentMethod::MobileMoney,
                "ussd" => PaymentMethod::Ussd,
                _ => PaymentMethod::Other,
            }),
            timestamp: raw.data.paid_at,
            failure_reason: raw.data.gateway_response,
            provider_data: None,
        })
    }

    async fn process_withdrawal(
        &self,
        request: WithdrawalRequest,
    ) -> PaymentResult<WithdrawalResponse> {
        request.amount.validate_positive("amount")?;
        if !matches!(request.withdrawal_method, WithdrawalMethod::BankTransfer) {
            return Err(PaymentError::ValidationError {
                message: "paystack currently supports bank transfer withdrawals only".to_string(),
                field: Some("withdrawal_method".to_string()),
            });
        }

        let account_name = request
            .recipient
            .account_name
            .clone()
            .unwrap_or_else(|| "Recipient".to_string());
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

        let recipient_payload = serde_json::json!({
            "type": "nuban",
            "name": account_name,
            "account_number": account_number,
            "bank_code": bank_code,
            "currency": request.amount.currency,
        });

        let recipient: PaystackEnvelope<PaystackRecipientData> = self
            .http
            .request_json(
                reqwest::Method::POST,
                &self.endpoint("/transferrecipient"),
                Some(&self.config.secret_key),
                Some(&recipient_payload),
                &[("Content-Type", "application/json")],
            )
            .await?;
        if !recipient.status {
            return Err(PaymentError::ProviderError {
                provider: "paystack".to_string(),
                message: recipient.message,
                provider_code: None,
                retryable: false,
            });
        }

        let transfer_payload = serde_json::json!({
            "source": "balance",
            "amount": request.amount.amount,
            "recipient": recipient.data.recipient_code,
            "reference": request.transaction_reference,
            "reason": request.reason,
            "metadata": request.metadata,
        });

        let transfer: PaystackEnvelope<PaystackTransferData> = self
            .http
            .request_json(
                reqwest::Method::POST,
                &self.endpoint("/transfer"),
                Some(&self.config.secret_key),
                Some(&transfer_payload),
                &[("Content-Type", "application/json")],
            )
            .await?;
        if !transfer.status {
            return Err(PaymentError::ProviderError {
                provider: "paystack".to_string(),
                message: transfer.message,
                provider_code: None,
                retryable: false,
            });
        }

        let status = match transfer.data.status.as_str() {
            "success" => PaymentState::Success,
            "pending" => PaymentState::Processing,
            "failed" => PaymentState::Failed,
            "reversed" => PaymentState::Reversed,
            _ => PaymentState::Unknown,
        };

        Ok(WithdrawalResponse {
            status,
            transaction_reference: request.transaction_reference,
            provider_reference: Some(transfer.data.reference),
            amount_debited: Some(request.amount),
            fees_charged: None,
            estimated_completion_seconds: Some(60),
            provider_data: Some(serde_json::json!({
                "transfer_code": transfer.data.transfer_code,
                "failure_reason": transfer.data.failure_reason
            })),
        })
    }

    async fn get_payment_status(&self, request: StatusRequest) -> PaymentResult<StatusResponse> {
        self.verify_payment(request).await
    }

    fn name(&self) -> ProviderName {
        ProviderName::Paystack
    }

    fn supported_currencies(&self) -> &'static [&'static str] {
        &["NGN", "GHS", "ZAR", "USD"]
    }

    fn supported_countries(&self) -> &'static [&'static str] {
        &["NG", "GH", "ZA"]
    }

    fn verify_webhook(
        &self,
        payload: &[u8],
        signature: &str,
    ) -> PaymentResult<WebhookVerificationResult> {
        let secret = self
            .config
            .webhook_secret
            .as_deref()
            .unwrap_or(&self.config.secret_key);
        let valid = verify_hmac_sha512_hex(payload, secret, signature);
        Ok(WebhookVerificationResult {
            valid,
            reason: if valid {
                None
            } else {
                Some("invalid paystack signature".to_string())
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
        let provider_ref = parsed
            .get("data")
            .and_then(|v| v.get("reference"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());
        let status = parsed
            .get("data")
            .and_then(|v| v.get("status"))
            .and_then(|v| v.as_str())
            .map(|v| match v {
                "success" => PaymentState::Success,
                "pending" => PaymentState::Pending,
                "failed" => PaymentState::Failed,
                _ => PaymentState::Unknown,
            });

        Ok(WebhookEvent {
            provider: ProviderName::Paystack,
            event_type,
            transaction_reference: None,
            provider_reference: provider_ref,
            status,
            payload: parsed,
            received_at: chrono::Utc::now().to_rfc3339(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct PaystackEnvelope<T> {
    status: bool,
    message: String,
    data: T,
}

#[derive(Debug, Deserialize)]
struct PaystackInitializeData {
    authorization_url: String,
    access_code: String,
    reference: String,
}

#[derive(Debug, Deserialize)]
struct PaystackVerifyData {
    amount: u64,
    currency: String,
    status: String,
    channel: String,
    #[serde(default)]
    paid_at: Option<String>,
    #[serde(default)]
    gateway_response: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PaystackRecipientData {
    recipient_code: String,
}

#[derive(Debug, Deserialize)]
struct PaystackTransferData {
    transfer_code: String,
    reference: String,
    status: String,
    #[serde(default)]
    failure_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> PaystackProvider {
        PaystackProvider::new(PaystackConfig {
            public_key: Some("pk_test".to_string()),
            secret_key: "sk_test".to_string(),
            webhook_secret: Some("whsec_test".to_string()),
            base_url: "https://api.paystack.co".to_string(),
            timeout_secs: 5,
            max_retries: 1,
        })
        .expect("provider init should succeed")
    }

    #[test]
    fn webhook_signature_validation_invalid() {
        let provider = provider();
        let payload = br#"{"event":"charge.success"}"#;
        let result = provider
            .verify_webhook(payload, "invalid_signature")
            .expect("verification should not error");
        assert!(!result.valid);
    }

    #[test]
    fn secure_eq_works() {
        assert!(crate::payments::utils::secure_eq(b"abc", b"abc"));
        assert!(!crate::payments::utils::secure_eq(b"abc", b"abd"));
    }
}
