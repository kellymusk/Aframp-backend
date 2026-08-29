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

    async fn verify_payout(&self, reference: &str) -> Result<PayoutVerification, String> {
        Ok(PayoutVerification::Pending {
            provider: "mock".into(),
            provider_reference: format!("mock_{reference}"),
        })
    }
}