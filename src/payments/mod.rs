pub mod mock;
pub mod paystack;

use async_trait::async_trait;

#[derive(Debug, Clone, serde::Serialize)]
pub struct PayoutRequest {
    pub bank_code: Option<String>,
    pub account_number: Option<String>,
    pub destination_address: Option<String>,
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

    async fn create_stellar_payout(&self, req: &PayoutRequest) -> Result<PayoutResult, String> {
        let _ = req;
        Err("stellar payouts are not supported by this provider".to_string())
    }
}