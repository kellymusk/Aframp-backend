pub mod mock;
pub mod paystack;

use async_trait::async_trait;

#[derive(Debug, Clone, serde::Serialize)]
pub struct PayoutRequest {
    pub bank_code: String,
    pub account_number: String,
    /// Smallest currency unit for the payout rail (e.g. kobo for a Naira payout).
    pub amount: String,
    pub reference: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PayoutResult {
    pub provider: String,
    pub provider_reference: String,
    pub status: String,
}

#[async_trait]
pub trait PaymentProvider: Send + Sync {
    async fn create_payout(&self, req: &PayoutRequest) -> Result<PayoutResult, String>;
}