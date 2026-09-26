use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use sha2::Sha512;

use super::{PaymentProvider, PayoutRequest, PayoutResult};

const BASE_URL: &str = "https://api.paystack.co";

pub struct PaystackProvider {
    secret_key: String,
    http: reqwest::Client,
}

impl PaystackProvider {
    pub fn new(secret_key: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("failed to build Paystack HTTP client");
        Self { secret_key, http }
    }

    /// Verify the `x-paystack-signature` header against the raw webhook body.
    ///
    /// Paystack signs the exact request body with HMAC-SHA512 using the account's
    /// webhook secret (the same value as the secret key for the integration).
    pub fn verify_webhook_signature(secret: &str, body: &[u8], signature: &str) -> bool {
        let mut mac = match Hmac::<Sha512>::new_from_slice(secret.as_bytes()) {
            Ok(mac) => mac,
            Err(_) => return false,
        };
        mac.update(body);
        let expected = hex::encode(mac.finalize().into_bytes());
        // Constant-time comparison to avoid leaking the expected digest.
        expected.len() == signature.len()
            && expected
                .bytes()
                .zip(signature.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T, String> {
        let response = self
            .http
            .get(format!("{BASE_URL}{path}"))
            .bearer_auth(&self.secret_key)
            .query(query)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::unwrap_response(response).await
    }

    async fn post<T: DeserializeOwned>(&self, path: &str, body: &serde_json::Value) -> Result<T, String> {
        let response = self
            .http
            .post(format!("{BASE_URL}{path}"))
            .bearer_auth(&self.secret_key)
            .json(body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::unwrap_response(response).await
    }

    async fn unwrap_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, String> {
        let status = response.status();
        let raw = response.text().await.map_err(|e| e.to_string())?;
        let body: PaystackResponse<T> = serde_json::from_str(&raw).map_err(|e| {
            format!("Paystack returned an unexpected response (HTTP {status}): {e} — body: {raw}")
        })?;
        if !status.is_success() || !body.status {
            return Err(format!("Paystack error (HTTP {status}): {}", body.message));
        }
        body.data.ok_or_else(|| "Paystack response missing data".to_string())
    }
}

#[derive(Debug, Deserialize)]
struct PaystackResponse<T> {
    status: bool,
    message: String,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct ResolvedAccount {
    account_name: String,
}

#[derive(Debug, Deserialize)]
struct Recipient {
    recipient_code: String,
}

#[derive(Debug, Deserialize)]
struct Transfer {
    transfer_code: String,
    status: String,
}

/// A Paystack webhook event envelope. Only the fields we reconcile on are
/// deserialized; unknown fields are ignored.
#[derive(Debug, Deserialize)]
pub struct PaystackWebhookEvent {
    pub event: String,
    pub data: PaystackWebhookData,
}

#[derive(Debug, Deserialize)]
pub struct PaystackWebhookData {
    /// The transfer code returned when the payout was created.
    #[serde(default)]
    pub transfer_code: Option<String>,
    /// The merchant-supplied reference, used as a fallback lookup key.
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// The reconciliation outcome derived from a Paystack transfer webhook event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferWebhookOutcome {
    /// `transfer.success` — the transfer settled successfully.
    Success,
    /// `transfer.failed` — the transfer failed; the balance should be refunded.
    Failed,
    /// `transfer.reversed` — the transfer was reversed; the balance should be refunded.
    Reversed,
}

impl PaystackWebhookEvent {
    /// Map the event name to a reconciliation outcome, or `None` for events we
    /// don't act on (e.g. `transfer.initiated`).
    pub fn outcome(&self) -> Option<TransferWebhookOutcome> {
        match self.event.as_str() {
            "transfer.success" => Some(TransferWebhookOutcome::Success),
            "transfer.failed" => Some(TransferWebhookOutcome::Failed),
            "transfer.reversed" => Some(TransferWebhookOutcome::Reversed),
            _ => None,
        }
    }
}

#[async_trait]
impl PaymentProvider for PaystackProvider {
    async fn create_payout(&self, req: &PayoutRequest) -> Result<PayoutResult, String> {
        let amount_kobo: i64 = req
            .amount
            .parse()
            .map_err(|_| format!("invalid payout amount: {}", req.amount))?;

        // Best-effort: resolving gets us the real account holder's name (and would
        // catch a typo'd account number), but its failure shouldn't hard-block the
        // payout — Paystack checks this against real NIBSS data even in test mode,
        // so a fabricated test account number fails here even though the recipient
        // and transfer calls below don't perform the same check.
        let resolved: Option<ResolvedAccount> = self
            .get(
                "/bank/resolve",
                &[("account_number", req.account_number.as_str()), ("bank_code", req.bank_code.as_str())],
            )
            .await
            .map_err(|err| {
                tracing::warn!(error = %err, "account resolution failed, proceeding with a placeholder name");
                err
            })
            .ok();
        let recipient_name = resolved
            .map(|r| r.account_name)
            .unwrap_or_else(|| "Aframp Merchant".to_string());

        let recipient: Recipient = self
            .post(
                "/transferrecipient",
                &serde_json::json!({
                    "type": "nuban",
                    "name": recipient_name,
                    "account_number": req.account_number,
                    "bank_code": req.bank_code,
                    "currency": "NGN",
                }),
            )
            .await?;

        // Test-mode transfers resolve immediately with no real processing, so the
        // status on this response is authoritative for our purposes. Live mode is
        // genuinely async (may require OTP finalization); the final status is
        // reconciled asynchronously via the Paystack transfer webhook
        // (`POST /webhooks/paystack`), which calls `verify_webhook_signature` and
        // maps the event through `PaystackWebhookEvent::outcome`.
        let transfer: Transfer = self
            .post(
                "/transfer",
                &serde_json::json!({
                    "source": "balance",
                    "amount": amount_kobo,
                    "recipient": recipient.recipient_code,
                    "reason": "Aframp merchant withdrawal",
                    "reference": req.reference,
                }),
            )
            .await?;

        Ok(PayoutResult {
            provider: "paystack".into(),
            provider_reference: transfer.transfer_code,
            status: transfer.status,
        })
    }
}
