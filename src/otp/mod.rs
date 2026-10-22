pub mod mock;
pub mod termii;

use async_trait::async_trait;
use std::time::Duration;

/// Error returned by [`OtpProvider::send_sms`].
///
/// Distinguishes transient failures (HTTP 5xx, network timeouts) that are
/// worth retrying from permanent ones (e.g. HTTP 400 invalid phone) that
/// will never succeed on retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtpSendError {
    /// A transient failure that may succeed if retried.
    Transient(String),
    /// A permanent failure that should not be retried.
    Permanent(String),
}

impl OtpSendError {
    /// Returns `true` if this error is worth retrying.
    pub fn is_transient(&self) -> bool {
        matches!(self, OtpSendError::Transient(_))
    }
}

impl std::fmt::Display for OtpSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OtpSendError::Transient(msg) => write!(f, "transient OTP send error: {msg}"),
            OtpSendError::Permanent(msg) => write!(f, "permanent OTP send error: {msg}"),
        }
    }
}

impl std::error::Error for OtpSendError {}

#[async_trait]
pub trait OtpProvider: Send + Sync {
    async fn send_sms(&self, phone: &str, message: &str) -> Result<(), OtpSendError>;

    /// Verifies an inbound delivery-status webhook actually came from this
    /// provider, given the raw request body and its signature header. Kept
    /// on the provider (not exposed as a bare secret elsewhere) so the
    /// signing key never leaves the code that owns it.
    fn verify_webhook_signature(&self, body: &[u8], signature: &str) -> bool;
}

/// Number of retries attempted for transient send failures.
const SEND_RETRIES: u32 = 1;
/// Delay between retries for transient send failures.
const SEND_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Sends an OTP verification code via the given provider, retrying once on
/// transient failures (HTTP 5xx, network timeouts) with a short delay.
/// Permanent failures (e.g. invalid phone number) are returned immediately.
pub async fn send_code(
    otp: &dyn OtpProvider,
    phone: &str,
    code: &str,
) -> Result<(), OtpSendError> {
    let message = format!("Your Aframp verification code is {code}. It expires in 10 minutes.");

    let mut attempt = 0;
    loop {
        match otp.send_sms(phone, &message).await {
            Ok(()) => return Ok(()),
            Err(err) if err.is_transient() && attempt < SEND_RETRIES => {
                attempt += 1;
                tokio::time::sleep(SEND_RETRY_DELAY).await;
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FlakyProvider {
        calls: AtomicUsize,
        fail_first: bool,
    }

    #[async_trait]
    impl OtpProvider for FlakyProvider {
        async fn send_sms(&self, _phone: &str, _message: &str) -> Result<(), OtpSendError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_first && n == 0 {
                Err(OtpSendError::Transient("network timeout".into()))
            } else {
                Ok(())
            }
        }

        fn verify_webhook_signature(&self, _body: &[u8], _signature: &str) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn retries_once_on_transient_failure() {
        let provider = FlakyProvider {
            calls: AtomicUsize::new(0),
            fail_first: true,
        };

        let result = send_code(&provider, "+2348000000000", "123456").await;

        assert!(result.is_ok());
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    }

    struct RejectingProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl OtpProvider for RejectingProvider {
        async fn send_sms(&self, _phone: &str, _message: &str) -> Result<(), OtpSendError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(OtpSendError::Permanent("invalid phone number".into()))
        }

        fn verify_webhook_signature(&self, _body: &[u8], _signature: &str) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn does_not_retry_a_permanent_failure() {
        let provider = RejectingProvider { calls: AtomicUsize::new(0) };
        let result = send_code(&provider, "+2348000000000", "123456").await;
        assert_eq!(result, Err(OtpSendError::Permanent("invalid phone number".into())));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}
