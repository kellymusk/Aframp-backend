use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha512;

use super::OtpProvider;

const BASE_URL: &str = "https://api.ng.termii.com/api";

/// Build the Termii Messaging send URL. Kept as a pure function so tests can
/// assert the API key is never placed in the URL or query string.
pub fn sms_send_url() -> String {
    format!("{BASE_URL}/sms/send")
}

/// The event body Termii POSTs to the dashboard-configured webhook URL for
/// SMS delivery status (sent/delivered/failed/etc.) — see
/// https://developers.termii.com/events-and-reports. Deserialized leniently:
/// every field optional, since the docs don't commit to which are always
/// present, and a webhook handler should never hard-fail on an unexpected
/// shape.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TermiiWebhookEvent {
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    pub id: Option<String>,
    pub message_id: Option<String>,
    pub receiver: Option<String>,
    pub sender: Option<String>,
    pub message: Option<String>,
    pub sent_at: Option<String>,
    pub cost: Option<String>,
    pub status: Option<String>,
    pub channel: Option<String>,
}

pub struct TermiiProvider {
    api_key: String,
    sender_id: String,
    http: reqwest::Client,
}

impl TermiiProvider {
    pub fn new(api_key: String, sender_id: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("failed to build Termii HTTP client");
        Self { api_key, sender_id, http }
    }
}

#[derive(Debug, Deserialize)]
struct SendResponse {
    code: Option<String>,
    message: Option<String>,
}

#[async_trait]
impl OtpProvider for TermiiProvider {
    async fn send_sms(&self, phone: &str, message: &str) -> Result<(), String> {
        // SECURITY (#1120 / #1122):
        // Termii's Messaging API requires `api_key` in the JSON body — their
        // docs do not support Bearer / Authorization-header auth for this
        // endpoint (see https://developers.termii.com/messaging-api). The
        // Token API has the same body-key requirement. We therefore cannot
        // move the secret to a header without breaking sends.
        //
        // Mitigations:
        // - Transport is always HTTPS (hard-coded base URL).
        // - The key is never placed in the URL or query string (see tests).
        // - Operators should disable request-body logging on any egress
        //   proxy/WAF/APM between this service and Termii.
        // - Prefer Termii's Token API as a follow-up for OTP lifecycle; it
        //   still needs the body key but avoids embedding the OTP in our
        //   own message-formatting path. See docs/SECURITY.md.
        let url = sms_send_url();
        debug_assert!(
            !url.contains(&self.api_key) && !url.contains('?'),
            "Termii send URL must never carry the API key or query params"
        );

        let response = self
            .http
            .post(&url)
            .json(&serde_json::json!({
                // Required by Termii — do not move to a query string.
                "api_key": self.api_key,
                "to": phone,
                "from": self.sender_id,
                "sms": message,
                "type": "plain",
                // The generic route is promotional-only and won't deliver to
                // numbers on Do-Not-Disturb; OTP/transactional traffic must
                // go over the DND route.
                "channel": "dnd",
            }))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let status = response.status();
        let raw = response.text().await.map_err(|e| e.to_string())?;
        let body: SendResponse = serde_json::from_str(&raw).map_err(|e| {
            format!("Termii returned an unexpected response (HTTP {status}): {e} — body: {raw}")
        })?;

        if !status.is_success() || body.code.as_deref() != Some("ok") {
            let message = body.message.unwrap_or_else(|| raw.clone());
            return Err(format!("Termii error (HTTP {status}): {message}"));
        }
        Ok(())
    }

    /// Termii signs webhook payloads as `HMAC-SHA512(secret, raw_body)`, hex
    /// encoded, in the `X-Termii-Signature` header. Their docs say "your
    /// secret key" without naming which one — the account API key is the
    /// only secret both sides are known to share, so that's what's used
    /// here. Worth double-checking against the dashboard's webhook config
    /// page if verification ever starts failing on genuine Termii traffic.
    fn verify_webhook_signature(&self, body: &[u8], signature: &str) -> bool {
        let Ok(sig_bytes) = hex::decode(signature) else {
            return false;
        };
        let Ok(mut mac) = Hmac::<Sha512>::new_from_slice(self.api_key.as_bytes()) else {
            return false;
        };
        mac.update(body);
        mac.verify_slice(&sig_bytes).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sms_send_url_does_not_embed_api_key_or_query() {
        let url = sms_send_url();
        let fake_key = "termii-secret-key-should-never-appear";
        assert!(
            !url.contains(fake_key),
            "send URL must not contain an API key: {url}"
        );
        assert!(
            !url.contains('?') && !url.contains("api_key"),
            "send URL must not use query params for auth: {url}"
        );
        assert!(
            url.starts_with("https://"),
            "Termii traffic must be HTTPS: {url}"
        );
        assert_eq!(url, "https://api.ng.termii.com/api/sms/send");
    }

    #[test]
    fn provider_send_url_never_includes_configured_key() {
        let key = "super-secret-termii-key-xyz";
        // Constructing the provider must not bake the key into any URL we log.
        let _provider = TermiiProvider::new(key.to_string(), "Aframp".into());
        let url = sms_send_url();
        assert!(
            !url.contains(key),
            "configured api_key must not appear in the request URL: {url}"
        );
    }
}
