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
    /// Secret that signs `X-Termii-Signature`. Termii's docs only say "your
    /// secret key", so this defaults to the API key (the one secret both
    /// sides are known to share) unless `TERMII_WEBHOOK_SECRET` is set.
    webhook_secret: Option<String>,
    http: reqwest::Client,
}

impl TermiiProvider {
    pub fn new(api_key: String, sender_id: String) -> Self {
        // Explicit connection-pool tuning for production use:
        // - pool_max_idle_per_host(5): cap idle keep-alive connections per
        //   host so a burst of traffic doesn't leave a large idle pool.
        // - connection_verbose(false): keep reqwest's per-connection debug
        //   logging off in production.
        // - tcp_keepalive(30s): keep long-lived connections alive through
        //   NAT/load balancers that drop idle sockets.
        // - connect_timeout(5s): bound only the TCP/TLS handshake, separate
        //   from the overall request timeout below.
        let http = reqwest::Client::builder()
            .pool_max_idle_per_host(5)
            .connection_verbose(false)
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("failed to build Termii HTTP client");
        Self { api_key, sender_id, webhook_secret: None, http }
    }

    /// Verify webhook signatures with a dedicated secret instead of the API key.
    pub fn with_webhook_secret(mut self, webhook_secret: Option<String>) -> Self {
        self.webhook_secret = webhook_secret;
        self
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
        let key = self.webhook_secret.as_deref().unwrap_or(&self.api_key);
        let Ok(mut mac) = Hmac::<Sha512>::new_from_slice(key.as_bytes()) else {
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

    const BODY: &[u8] = br#"{"type":"outbound","message_id":"abc123","status":"Delivered"}"#;
    /// HMAC-SHA512 of `BODY` keyed with `termii-api-key`.
    const SIG_API_KEY: &str = "23dedc762dd410473d46b6ea8d5cf5464110861e422fba864176f2ed4edb59ab8e30429943bc7a752da747372ee12a112d544535847289b159381656e36078c3";
    /// HMAC-SHA512 of `BODY` keyed with `termii-webhook-secret`.
    const SIG_WEBHOOK_SECRET: &str = "0e4eb03abda351b2e5f8d5e750b4bdd5d1c7a46b8fdf887ba179428986099c1b7ebeaa3d451717d504e9ebcc89ea550ab4364b54f80e0d1f46db270ae5dadae2";

    fn provider() -> TermiiProvider {
        TermiiProvider::new("termii-api-key".into(), "Aframp".into())
    }

    #[test]
    fn accepts_known_good_signature_keyed_with_api_key() {
        assert!(provider().verify_webhook_signature(BODY, SIG_API_KEY));
    }

    #[test]
    fn rejects_signature_for_a_different_body() {
        let tampered = br#"{"type":"outbound","message_id":"abc123","status":"Failed"}"#;
        assert!(!provider().verify_webhook_signature(tampered, SIG_API_KEY));
    }

    #[test]
    fn rejects_missing_and_non_hex_signatures() {
        assert!(!provider().verify_webhook_signature(BODY, ""));
        assert!(!provider().verify_webhook_signature(BODY, "not-hex"));
    }

    #[test]
    fn dedicated_webhook_secret_replaces_api_key() {
        let p = provider().with_webhook_secret(Some("termii-webhook-secret".into()));
        assert!(p.verify_webhook_signature(BODY, SIG_WEBHOOK_SECRET));
        assert!(!p.verify_webhook_signature(BODY, SIG_API_KEY));
    }

    #[test]
    fn unset_webhook_secret_falls_back_to_api_key() {
        let p = provider().with_webhook_secret(None);
        assert!(p.verify_webhook_signature(BODY, SIG_API_KEY));
        assert!(!p.verify_webhook_signature(BODY, SIG_WEBHOOK_SECRET));
    }
}
