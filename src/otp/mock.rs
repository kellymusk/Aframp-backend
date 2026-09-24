use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use super::OtpProvider;

static SENT: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

pub struct MockOtpProvider;

#[async_trait]
impl OtpProvider for MockOtpProvider {
    async fn send_sms(&self, phone: &str, message: &str) -> Result<(), String> {
        tracing::info!(phone, message, "mock OTP send (no real SMS delivered)");
        SENT.lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(phone.to_string(), message.to_string());
        Ok(())
    }

    /// No real signing in tests — a fixed sentinel stands in for "valid".
    fn verify_webhook_signature(&self, _body: &[u8], signature: &str) -> bool {
        signature == "mock-signature"
    }
}

/// The message body most recently "sent" to `phone`, for tests to pull the
/// code back out of without a live Termii account.
pub fn last_message_for(phone: &str) -> Option<String> {
    SENT.lock().unwrap().as_ref()?.get(phone).cloned()
}
