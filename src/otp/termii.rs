use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha512;

use super::OtpProvider;

const BASE_URL: &str = "https://api.ng.termii.com/api";

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
        let response = self
            .http
            .post(format!("{BASE_URL}/sms/send"))
            .json(&serde_json::json!({
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
