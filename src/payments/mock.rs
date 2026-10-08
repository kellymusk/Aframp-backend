use async_trait::async_trait;

use super::{PaymentProvider, PayoutRequest, PayoutResult, PayoutVerification};

pub struct MockProvider;

#[async_trait]
impl PaymentProvider for MockProvider {
    async fn create_payout(&self, req: &PayoutRequest) -> Result<PayoutResult, String> {
        Ok(PayoutResult {
            provider: "mock".into(),
            provider_reference: format!("mock_{}", req.reference),
            status: "pending".into(),
        })
    }

    /// No real signing in tests — a fixed sentinel stands in for "valid".
    fn verify_webhook_signature(&self, _body: &[u8], signature: &str) -> bool {
        signature == "mock-signature"
    }

    async fn resolve_account(&self, _bank_code: &str, _account_number: &str) -> Result<String, String> {
        Ok("Mock Account Holder".into())
    }

    async fn verify_payout(&self, reference: &str) -> Result<PayoutVerification, String> {
        Ok(PayoutVerification::Pending {
            provider: "mock".into(),
            provider_reference: format!("mock_{reference}"),
        })
    }
}
