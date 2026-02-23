use crate::payments::error::PaymentError;
use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ProviderName {
    Paystack,
    Flutterwave,
    Mpesa,
}

impl ProviderName {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderName::Paystack => "paystack",
            ProviderName::Flutterwave => "flutterwave",
            ProviderName::Mpesa => "mpesa",
        }
    }
}

impl std::fmt::Display for ProviderName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for ProviderName {
    type Err = PaymentError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_lowercase().as_str() {
            "paystack" => Ok(ProviderName::Paystack),
            "flutterwave" => Ok(ProviderName::Flutterwave),
            "mpesa" | "m-pesa" => Ok(ProviderName::Mpesa),
            _ => Err(PaymentError::ValidationError {
                message: format!("unsupported provider: {}", value),
                field: Some("provider".to_string()),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Money {
    pub amount: String,
    pub currency: String,
}

impl Money {
    pub fn validate_positive(&self, field: &str) -> Result<(), PaymentError> {
        let parsed =
            BigDecimal::from_str(&self.amount).map_err(|_| PaymentError::ValidationError {
                message: format!("invalid decimal amount: {}", self.amount),
                field: Some(field.to_string()),
            })?;
        if parsed <= BigDecimal::from(0) {
            return Err(PaymentError::ValidationError {
                message: "amount must be greater than zero".to_string(),
                field: Some(field.to_string()),
            });
        }
        if self.currency.trim().is_empty() {
            return Err(PaymentError::ValidationError {
                message: "currency is required".to_string(),
                field: Some("currency".to_string()),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaymentMethod {
    Card,
    BankTransfer,
    MobileMoney,
    Ussd,
    Wallet,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawalMethod {
    BankTransfer,
    MobileMoney,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaymentState {
    Pending,
    Processing,
    Success,
    Failed,
    Cancelled,
    Reversed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomerContact {
    pub email: Option<String>,
    pub phone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawalRecipient {
    pub account_name: Option<String>,
    pub account_number: Option<String>,
    pub bank_code: Option<String>,
    pub phone_number: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentRequest {
    pub amount: Money,
    pub customer: CustomerContact,
    pub payment_method: PaymentMethod,
    pub callback_url: Option<String>,
    pub transaction_reference: String,
    pub metadata: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawalRequest {
    pub amount: Money,
    pub recipient: WithdrawalRecipient,
    pub withdrawal_method: WithdrawalMethod,
    pub transaction_reference: String,
    pub reason: Option<String>,
    pub metadata: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusRequest {
    pub transaction_reference: Option<String>,
    pub provider_reference: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentResponse {
    pub status: PaymentState,
    pub transaction_reference: String,
    pub provider_reference: Option<String>,
    pub payment_url: Option<String>,
    pub amount_charged: Option<Money>,
    pub fees_charged: Option<Money>,
    pub provider_data: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawalResponse {
    pub status: PaymentState,
    pub transaction_reference: String,
    pub provider_reference: Option<String>,
    pub amount_debited: Option<Money>,
    pub fees_charged: Option<Money>,
    pub estimated_completion_seconds: Option<u64>,
    pub provider_data: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    pub status: PaymentState,
    pub transaction_reference: Option<String>,
    pub provider_reference: Option<String>,
    pub amount: Option<Money>,
    pub payment_method: Option<PaymentMethod>,
    pub timestamp: Option<String>,
    pub failure_reason: Option<String>,
    pub provider_data: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookVerificationResult {
    pub valid: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEvent {
    pub provider: ProviderName,
    pub event_type: String,
    pub transaction_reference: Option<String>,
    pub provider_reference: Option<String>,
    pub status: Option<PaymentState>,
    pub payload: JsonValue,
    pub received_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payment_request_serializes_to_json() {
        let request = PaymentRequest {
            amount: Money {
                amount: "1000.00".to_string(),
                currency: "NGN".to_string(),
            },
            customer: CustomerContact {
                email: Some("user@example.com".to_string()),
                phone: Some("+2348012345678".to_string()),
            },
            payment_method: PaymentMethod::Card,
            callback_url: Some("https://example.com/callback".to_string()),
            transaction_reference: "txn_ref_1".to_string(),
            metadata: Some(serde_json::json!({"user_id":"u1"})),
        };
        let json = serde_json::to_value(&request).expect("serialization should succeed");
        assert_eq!(json["amount"]["currency"], "NGN");
        assert_eq!(json["transaction_reference"], "txn_ref_1");
    }

    #[test]
    fn status_response_deserializes_from_json() {
        let payload = serde_json::json!({
            "status": "success",
            "transaction_reference": "txn_ref_1",
            "provider_reference": "ps_ref_1",
            "amount": {"amount":"1000.00","currency":"NGN"},
            "payment_method": "card",
            "timestamp": "2026-02-12T00:00:00Z",
            "failure_reason": null,
            "provider_data": {"key":"value"}
        });
        let parsed: StatusResponse =
            serde_json::from_value(payload).expect("deserialization should succeed");
        assert_eq!(parsed.status, PaymentState::Success);
        assert_eq!(parsed.provider_reference.as_deref(), Some("ps_ref_1"));
    }
}
