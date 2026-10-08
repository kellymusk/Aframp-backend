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

    /// Resolve a bank account to its registered holder name. A failure means
    /// the account could not be verified and no payout may be attempted.
    async fn resolve_account(&self, bank_code: &str, account_number: &str) -> Result<String, String>;

    /// Whether `signature` is the provider's valid signature over a webhook
    /// `body`. Lives on the provider so the signing key never leaves it.
    /// Providers that don't send webhooks reject every signature.
    fn verify_webhook_signature(&self, _body: &[u8], _signature: &str) -> bool {
        false
    }
}