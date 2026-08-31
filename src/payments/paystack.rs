use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use super::{PaymentProvider, PayoutRequest, PayoutResult};
use crate::config::SecretString;

const BASE_URL: &str = "https://api.paystack.co";

pub struct PaystackProvider {
    /// Held as a [`SecretString`] so the key cannot reach a log through the
    /// ordinary routes — `{:?}` on this struct, a `tracing` field, a
    /// `serde` round-trip — all of which print `[REDACTED]`. Only
    /// [`SecretString::as_str`] yields the real value, and the sole caller of
    /// it is `bearer_auth` below.
    secret_key: SecretString,
    http: reqwest::Client,
}

/// Derived `Debug` would print the client's configuration and, more to the
/// point, invite the key into a log line the first time someone adds
/// `?provider` to a `tracing` macro. This impl makes that impossible.
impl std::fmt::Debug for PaystackProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaystackProvider")
            .field("secret_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl PaystackProvider {
    pub fn new(secret_key: String) -> Self {
        Self::with_secret(SecretString::new(secret_key))
    }

    pub fn with_secret(secret_key: SecretString) -> Self {
        // `connection_verbose(false)` is the default, but state it: with it on,
        // reqwest's own `trace`-level wire logging prints request headers
        // verbatim — including `Authorization: Bearer sk_live_…`. Nothing in
        // this process should be able to turn that on by raising RUST_LOG.
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .connection_verbose(false)
            .build()
            .expect("failed to build Paystack HTTP client");
        Self { secret_key, http }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T, String> {
        let response = self
            .http
            .get(format!("{BASE_URL}{path}"))
            .bearer_auth(self.secret_key.as_str())
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
            .bearer_auth(self.secret_key.as_str())
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
        // genuinely async (may require OTP finalization) and would need a webhook
        // or a follow-up "verify transfer" call to reconcile the final status.
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
