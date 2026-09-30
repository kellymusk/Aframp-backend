use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::OtpProvider;

/// In-memory OTP provider used when `OTP_PROVIDER=mock`.
///
/// Sent messages are recorded in a thread-safe store so tests can read back
/// the OTP code that would have been delivered via SMS and complete the
/// signup/login verification flow end-to-end.
#[derive(Clone, Default)]
pub struct MockOtpProvider {
    sent: Arc<Mutex<Vec<SentMessage>>>,
}

#[derive(Clone, Debug)]
pub struct SentMessage {
    pub phone: String,
    pub message: String,
}

impl MockOtpProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns every message sent through this provider, oldest first.
    pub fn sent_messages(&self) -> Vec<SentMessage> {
        self.sent.lock().expect("mock otp store poisoned").clone()
    }

    /// Returns the most recent message sent to `phone`, if any.
    pub fn last_message_for(&self, phone: &str) -> Option<SentMessage> {
        self.sent
            .lock()
            .expect("mock otp store poisoned")
            .iter()
            .rev()
            .find(|m| m.phone == phone)
            .cloned()
    }

    /// Extracts the numeric OTP code from the most recent message sent to
    /// `phone`. Codes are the first run of 4-8 digits found in the message.
    pub fn last_code_for(&self, phone: &str) -> Option<String> {
        let message = self.last_message_for(phone)?;
        extract_code(&message.message)
    }

    /// Clears all captured messages. Useful between tests sharing a provider.
    pub fn clear(&self) {
        self.sent.lock().expect("mock otp store poisoned").clear();
    }
}

fn extract_code(message: &str) -> Option<String> {
    let mut digits = String::new();
    for ch in message.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if !digits.is_empty() {
            if (4..=8).contains(&digits.len()) {
                return Some(digits);
            }
            digits.clear();
        }
    }
    if (4..=8).contains(&digits.len()) {
        Some(digits)
    } else {
        None
    }
}

#[async_trait]
impl OtpProvider for MockOtpProvider {
    async fn send_sms(&self, phone: &str, message: &str) -> Result<(), String> {
        tracing::info!(phone = %phone, message = %message, "mock OTP sent");
        self.sent
            .lock()
            .expect("mock otp store poisoned")
            .push(SentMessage {
                phone: phone.to_string(),
                message: message.to_string(),
            });
        Ok(())
    }

    fn verify_webhook_signature(&self, _body: &[u8], _signature: &str) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_code_from_message() {
        assert_eq!(
            extract_code("Your verification code is 123456"),
            Some("123456".to_string())
        );
        assert_eq!(extract_code("no code here"), None);
    }

    #[tokio::test]
    async fn captures_sent_codes() {
        let provider = MockOtpProvider::new();
        provider
            .send_sms("+2348000000000", "Your code is 654321")
            .await
            .unwrap();
        assert_eq!(
            provider.last_code_for("+2348000000000"),
            Some("654321".to_string())
        );
        assert_eq!(provider.sent_messages().len(), 1);
        provider.clear();
        assert!(provider.sent_messages().is_empty());
    }
}

/// Extract the numeric OTP code from a message body. The mock provider stores
/// the full SMS text, so tests need a way to recover just the code that the
/// onboarding flow expects the user to submit.
pub fn extract_code(message: &str) -> Option<String> {
    message
        .split(|c: char| !c.is_ascii_digit())
        .find(|token| token.len() >= 4 && token.len() <= 8)
        .map(|token| token.to_string())
}

/// Convenience helper for the end-to-end onboarding test: returns the OTP code
/// most recently "sent" to `phone`, if any.
pub fn last_code_for(phone: &str) -> Option<String> {
    last_message_for(phone).and_then(|message| extract_code(&message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_code_pulls_digits_out_of_message() {
        assert_eq!(
            extract_code("Your verification code is 482913").as_deref(),
            Some("482913")
        );
    }

    #[test]
    fn extract_code_returns_none_without_a_code() {
        assert_eq!(extract_code("no digits here"), None);
    }
}
