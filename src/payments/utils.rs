use crate::payments::error::{PaymentError, PaymentResult};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use std::time::Duration;
use tracing::warn;

#[derive(Clone)]
pub struct PaymentHttpClient {
    client: Client,
    timeout: Duration,
    max_retries: u32,
}

impl PaymentHttpClient {
    pub fn new(timeout: Duration, max_retries: u32) -> PaymentResult<Self> {
        let client =
            Client::builder()
                .timeout(timeout)
                .build()
                .map_err(|e| PaymentError::NetworkError {
                    message: format!("failed to initialize HTTP client: {}", e),
                })?;

        Ok(Self {
            client,
            timeout,
            max_retries,
        })
    }

    pub async fn request_json<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        url: &str,
        bearer_token: Option<&str>,
        body: Option<&JsonValue>,
        additional_headers: &[(&str, &str)],
    ) -> PaymentResult<T> {
        let mut last_error = None;
        for attempt in 0..=self.max_retries {
            let mut request = self.client.request(method.clone(), url);
            request = request.timeout(self.timeout);

            if let Some(token) = bearer_token {
                request = request.bearer_auth(token);
            }
            for (k, v) in additional_headers {
                request = request.header(*k, *v);
            }
            if let Some(payload) = body {
                request = request.json(payload);
            }

            let response = request
                .send()
                .await
                .map_err(|e| PaymentError::NetworkError {
                    message: format!("provider request failed: {}", e),
                });

            match response {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        return serde_json::from_str::<T>(&text).map_err(|e| {
                            PaymentError::ProviderError {
                                provider: "http".to_string(),
                                message: format!("invalid provider JSON response: {}", e),
                                provider_code: None,
                                retryable: false,
                            }
                        });
                    }

                    if status.as_u16() == 429 {
                        if attempt < self.max_retries {
                            tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                            continue;
                        }
                        return Err(PaymentError::RateLimitError {
                            message: "provider rate limit exceeded".to_string(),
                            retry_after_seconds: None,
                        });
                    }

                    if status.is_server_error() && attempt < self.max_retries {
                        warn!(
                            status = %status,
                            attempt = attempt + 1,
                            "provider server error, retrying"
                        );
                        tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                        continue;
                    }

                    return Err(PaymentError::ProviderError {
                        provider: "http".to_string(),
                        message: format!("HTTP {}: {}", status, text),
                        provider_code: Some(status.as_u16().to_string()),
                        retryable: status.is_server_error(),
                    });
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < self.max_retries {
                        tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                        continue;
                    }
                }
            }
        }

        Err(last_error.unwrap_or(PaymentError::NetworkError {
            message: "provider request failed".to_string(),
        }))
    }
}

pub fn verify_hmac_sha512_hex(payload: &[u8], secret: &str, signature: &str) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha512;

    type HmacSha512 = Hmac<Sha512>;
    let mut mac = match HmacSha512::new_from_slice(secret.as_bytes()) {
        Ok(v) => v,
        Err(_) => return false,
    };
    mac.update(payload);
    let computed = hex::encode(mac.finalize().into_bytes());
    secure_eq(computed.as_bytes(), signature.trim().as_bytes())
}

pub fn secure_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0_u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_eq_behaves_correctly() {
        assert!(secure_eq(b"abc", b"abc"));
        assert!(!secure_eq(b"abc", b"abd"));
        assert!(!secure_eq(b"abc", b"ab"));
    }

    #[test]
    fn webhook_hmac_verification_detects_invalid_signature() {
        let payload = br#"{"event":"charge.success"}"#;
        let valid = verify_hmac_sha512_hex(payload, "secret", "not-a-valid-signature");
        assert!(!valid);
    }
}
