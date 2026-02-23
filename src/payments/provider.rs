use crate::payments::error::PaymentResult;
use crate::payments::types::{
    PaymentRequest, PaymentResponse, ProviderName, StatusRequest, StatusResponse, WebhookEvent,
    WebhookVerificationResult, WithdrawalRequest, WithdrawalResponse,
};
use async_trait::async_trait;

#[async_trait]
pub trait PaymentProvider: Send + Sync {
    async fn initiate_payment(&self, request: PaymentRequest) -> PaymentResult<PaymentResponse>;

    async fn verify_payment(&self, request: StatusRequest) -> PaymentResult<StatusResponse>;

    async fn process_withdrawal(
        &self,
        request: WithdrawalRequest,
    ) -> PaymentResult<WithdrawalResponse>;

    async fn get_payment_status(&self, request: StatusRequest) -> PaymentResult<StatusResponse>;

    fn name(&self) -> ProviderName;

    fn supported_currencies(&self) -> &'static [&'static str];

    fn supported_countries(&self) -> &'static [&'static str];

    fn verify_webhook(
        &self,
        payload: &[u8],
        signature: &str,
    ) -> PaymentResult<WebhookVerificationResult>;

    fn parse_webhook_event(&self, payload: &[u8]) -> PaymentResult<WebhookEvent>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payments::types::{
        Money, PaymentMethod, PaymentState, ProviderName, WithdrawalMethod, WithdrawalRecipient,
    };

    struct MockProvider;

    #[async_trait]
    impl PaymentProvider for MockProvider {
        async fn initiate_payment(
            &self,
            request: PaymentRequest,
        ) -> PaymentResult<PaymentResponse> {
            Ok(PaymentResponse {
                status: PaymentState::Pending,
                transaction_reference: request.transaction_reference,
                provider_reference: Some("mock_ref".to_string()),
                payment_url: Some("https://example.com/pay".to_string()),
                amount_charged: Some(request.amount),
                fees_charged: None,
                provider_data: None,
            })
        }

        async fn verify_payment(&self, request: StatusRequest) -> PaymentResult<StatusResponse> {
            Ok(StatusResponse {
                status: PaymentState::Success,
                transaction_reference: request.transaction_reference,
                provider_reference: request.provider_reference,
                amount: None,
                payment_method: Some(PaymentMethod::Card),
                timestamp: None,
                failure_reason: None,
                provider_data: None,
            })
        }

        async fn process_withdrawal(
            &self,
            request: WithdrawalRequest,
        ) -> PaymentResult<WithdrawalResponse> {
            Ok(WithdrawalResponse {
                status: PaymentState::Processing,
                transaction_reference: request.transaction_reference,
                provider_reference: Some("mock_wd_ref".to_string()),
                amount_debited: Some(request.amount),
                fees_charged: None,
                estimated_completion_seconds: Some(10),
                provider_data: None,
            })
        }

        async fn get_payment_status(
            &self,
            request: StatusRequest,
        ) -> PaymentResult<StatusResponse> {
            self.verify_payment(request).await
        }

        fn name(&self) -> ProviderName {
            ProviderName::Paystack
        }

        fn supported_currencies(&self) -> &'static [&'static str] {
            &["NGN"]
        }

        fn supported_countries(&self) -> &'static [&'static str] {
            &["NG"]
        }

        fn verify_webhook(
            &self,
            _payload: &[u8],
            _signature: &str,
        ) -> PaymentResult<WebhookVerificationResult> {
            Ok(WebhookVerificationResult {
                valid: true,
                reason: None,
            })
        }

        fn parse_webhook_event(&self, _payload: &[u8]) -> PaymentResult<WebhookEvent> {
            Ok(WebhookEvent {
                provider: ProviderName::Paystack,
                event_type: "mock".to_string(),
                transaction_reference: None,
                provider_reference: None,
                status: Some(PaymentState::Success),
                payload: serde_json::json!({}),
                received_at: chrono::Utc::now().to_rfc3339(),
            })
        }
    }

    #[tokio::test]
    async fn trait_can_be_implemented_by_mock_provider() {
        let provider: Box<dyn PaymentProvider> = Box::new(MockProvider);
        let payment_response = provider
            .initiate_payment(PaymentRequest {
                amount: Money {
                    amount: "1000".to_string(),
                    currency: "NGN".to_string(),
                },
                customer: crate::payments::types::CustomerContact {
                    email: Some("test@example.com".to_string()),
                    phone: None,
                },
                payment_method: PaymentMethod::Card,
                callback_url: None,
                transaction_reference: "txn_1".to_string(),
                metadata: None,
            })
            .await
            .expect("payment initiation should succeed");
        assert_eq!(payment_response.status, PaymentState::Pending);

        let withdrawal_response = provider
            .process_withdrawal(WithdrawalRequest {
                amount: Money {
                    amount: "500".to_string(),
                    currency: "NGN".to_string(),
                },
                recipient: WithdrawalRecipient {
                    account_name: Some("Test".to_string()),
                    account_number: Some("0123456789".to_string()),
                    bank_code: Some("058".to_string()),
                    phone_number: None,
                },
                withdrawal_method: WithdrawalMethod::BankTransfer,
                transaction_reference: "wd_1".to_string(),
                reason: None,
                metadata: None,
            })
            .await
            .expect("withdrawal should succeed");
        assert_eq!(withdrawal_response.status, PaymentState::Processing);
    }
}
