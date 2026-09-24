pub mod mock;
pub mod termii;

use async_trait::async_trait;

#[async_trait]
pub trait OtpProvider: Send + Sync {
    async fn send_sms(&self, phone: &str, message: &str) -> Result<(), String>;

    /// Verifies an inbound delivery-status webhook actually came from this
    /// provider, given the raw request body and its signature header. Kept
    /// on the provider (not exposed as a bare secret elsewhere) so the
    /// signing key never leaves the code that owns it.
    fn verify_webhook_signature(&self, body: &[u8], signature: &str) -> bool;
}
