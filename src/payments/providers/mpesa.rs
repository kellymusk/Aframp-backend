use crate::payments::error::{PaymentError, PaymentResult};
use crate::payments::provider::PaymentProvider;
use crate::payments::types::{
    PaymentRequest, PaymentResponse, PaymentState, ProviderName, StatusRequest, StatusResponse,
    WebhookEvent, WebhookVerificationResult, WithdrawalRequest, WithdrawalResponse,
};
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct MpesaConfig {
    pub consumer_key: String,
    pub consumer_secret: String,
    pub passkey: String,
}

impl MpesaConfig {
    pub fn from_env() -> PaymentResult<Self> {
        let consumer_key = std::env::var("MPESA_CONSUMER_KEY").unwrap_or_default();
        let consumer_secret = std::env::var("MPESA_CONSUMER_SECRET").unwrap_or_default();
        let passkey = std::env::var("MPESA_PASSKEY").unwrap_or_default();
        if consumer_key.is_empty() || consumer_secret.is_empty() || passkey.is_empty() {
            return Err(PaymentError::ValidationError {
                message: "MPESA_CONSUMER_KEY, MPESA_CONSUMER_SECRET and MPESA_PASSKEY are required"
                    .to_string(),
                field: Some("mpesa".to_string()),
            });
        }
        Ok(Self {
            consumer_key,
            consumer_secret,
            passkey,
        })
    }
}

pub struct MpesaProvider {
    _config: MpesaConfig,
}

impl MpesaProvider {
    pub fn from_env() -> PaymentResult<Self> {
        Ok(Self {
            _config: MpesaConfig::from_env()?,
        })
    }
}

#[async_trait]
impl PaymentProvider for MpesaProvider {
    async fn initiate_payment(&self, _request: PaymentRequest) -> PaymentResult<PaymentResponse> {
        Err(PaymentError::ProviderError {
            provider: "mpesa".to_string(),
            message: "not implemented yet".to_string(),
            provider_code: None,
            retryable: false,
        })
    }

    async fn verify_payment(&self, _request: StatusRequest) -> PaymentResult<StatusResponse> {
        Err(PaymentError::ProviderError {
            provider: "mpesa".to_string(),
            message: "not implemented yet".to_string(),
            provider_code: None,
            retryable: false,
        })
    }

    async fn process_withdrawal(
        &self,
        _request: WithdrawalRequest,
    ) -> PaymentResult<WithdrawalResponse> {
        Err(PaymentError::ProviderError {
            provider: "mpesa".to_string(),
            message: "not implemented yet".to_string(),
            provider_code: None,
            retryable: false,
        })
    }

    async fn get_payment_status(&self, request: StatusRequest) -> PaymentResult<StatusResponse> {
        self.verify_payment(request).await
    }

    fn name(&self) -> ProviderName {
        ProviderName::Mpesa
    }

    fn supported_currencies(&self) -> &'static [&'static str] {
        &["KES", "TZS", "UGX"]
    }

    fn supported_countries(&self) -> &'static [&'static str] {
        &["KE", "TZ", "UG"]
    }

    fn verify_webhook(
        &self,
        _payload: &[u8],
        _signature: &str,
    ) -> PaymentResult<WebhookVerificationResult> {
        Ok(WebhookVerificationResult {
            valid: false,
            reason: Some("not implemented yet".to_string()),
        })
    }

    fn parse_webhook_event(&self, payload: &[u8]) -> PaymentResult<WebhookEvent> {
        let parsed = serde_json::from_slice(payload).unwrap_or_else(|_| serde_json::json!({}));
        Ok(WebhookEvent {
            provider: ProviderName::Mpesa,
            event_type: "unknown".to_string(),
            transaction_reference: None,
            provider_reference: None,
            status: Some(PaymentState::Unknown),
            payload: parsed,
            received_at: chrono::Utc::now().to_rfc3339(),
        })
    }
}
