use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use super::OtpProvider;

static SENT: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

pub struct MockOtpProvider;

#[async_trait]
impl OtpProvider for MockOtpProvider {
    async fn send_sms(&self, phone: &str, message: &str) -> Result<(), super::OtpSendError> {
        #[cfg(not(test))]
        tracing::debug!(
            phone,
            message = %mask_digits(message),
            "mock OTP send (no real SMS delivered)"
        );
        #[cfg(test)]
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

/// Mask the digits in a message body so OTP codes are never logged in
/// plaintext. Keeps the first two digits (useful for correlating a log line
/// with a specific send) and replaces every remaining digit with `*`.
pub fn mask_digits(message: &str) -> String {
    let mut seen = 0usize;
    message
        .chars()
        .map(|c| {
            if c.is_ascii_digit() {
                seen += 1;
                if seen <= 2 {
                    c
                } else {
                    '*'
                }
            } else {
                c
            }
        })
        .collect()
}

/// The message body most recently "sent" to `phone`, for tests to pull the
/// code back out of without a live Termii account.
pub fn last_message_for(phone: &str) -> Option<String> {
    SENT.lock().unwrap().as_ref()?.get(phone).cloned()
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

/// Convenience helper for end-to-end tests: returns the OTP code most
/// recently "sent" to `phone`, if any.
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

    #[test]
    fn mask_digits_hides_otp_code() {
        assert_eq!(
            mask_digits("Your verification code is 482913"),
            "Your verification code is 48****"
        );
    }

    #[test]
    fn mask_digits_keeps_short_numbers_visible() {
        assert_eq!(mask_digits("code 42"), "code 42");
    }

    #[test]
    fn mask_digits_leaves_non_digits_untouched() {
        assert_eq!(mask_digits("no digits here"), "no digits here");
    }
}
