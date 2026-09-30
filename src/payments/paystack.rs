use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;

use super::{PaymentProvider, PayoutRequest, PayoutResult};

const BASE_URL: &str = "https://api.paystack.co";

pub struct PaystackProvider {
    secret_key: String,
    http: reqwest::Client,
    // Cache of resolved account names keyed by (account_number, bank_code) so that
    // repeated withdrawals to the same account don't re-hit the resolution API.
    resolved_cache: Mutex<HashMap<(String, String), String>>,
}

impl PaystackProvider {
    pub fn new(secret_key: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("failed to build Paystack HTTP client");
        Self {
            secret_key,
            http,
            resolved_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Build a provider pointed at an arbitrary base URL. Used by contract tests
    /// to target a local mock server instead of the live Paystack API.
    #[cfg(test)]
    pub(crate) fn with_base_url(secret_key: String, base_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("failed to build Paystack HTTP client");
        Self {
            secret_key,
            http,
            base_url,
        }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T, String> {
        let response = self
            .http
            .get(format!("{}{path}", self.base_url))
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
            .post(format!("{}{path}", self.base_url))
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

    /// Resolve a bank account to its registered account holder name.
    ///
    /// This is a hard requirement before creating a withdrawal recipient: a
    /// failed resolution means the account number is likely wrong, and we must
    /// not attempt a transfer (which would debit the balance) with a placeholder
    /// name. Successful resolutions are cached so repeated withdrawals to the
    /// same account avoid redundant API calls.
    pub async fn verify_bank_account(
        &self,
        account_number: &str,
        bank_code: &str,
    ) -> Result<String, String> {
        let key = (account_number.to_string(), bank_code.to_string());
        if let Some(name) = self
            .resolved_cache
            .lock()
            .expect("resolved cache poisoned")
            .get(&key)
            .cloned()
        {
            return Ok(name);
        }

        let resolved: ResolvedAccount = self
            .get(
                "/bank/resolve",
                &[("account_number", account_number), ("bank_code", bank_code)],
            )
            .await?;

        self.resolved_cache
            .lock()
            .expect("resolved cache poisoned")
            .insert(key, resolved.account_name.clone());

        Ok(resolved.account_name)
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

        // Hard requirement: resolve the account before creating the recipient or
        // attempting the transfer. If resolution fails the account number is
        // likely invalid, so we fail fast and let the caller refund/abort before
        // any balance is debited.
        let recipient_name = self
            .verify_bank_account(&req.account_number, &req.bank_code)
            .await
            .map_err(|err| {
                format!(
                    "bank account verification failed for {} ({}): {err}",
                    req.account_number, req.bank_code
                )
            })?;

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

#[cfg(test)]
mod contract_tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Recorded Paystack API responses (test mode). These fixtures mirror the
    // exact JSON shape Paystack returns so a schema change breaks the build.
    const RESOLVE_RESPONSE: &str = r#"{
        "status": true,
        "message": "Account number resolved",
        "data": {
            "account_number": "0001234567",
            "account_name": "JOHN DOE",
            "bank_id": 9
        }
    }"#;

    const RECIPIENT_RESPONSE: &str = r#"{
        "status": true,
        "message": "Transfer recipient created successfully",
        "data": {
            "active": true,
            "createdAt": "2024-01-01T00:00:00.000Z",
            "currency": "NGN",
            "domain": "test",
            "id": 12345,
            "name": "JOHN DOE",
            "recipient_code": "RCP_abc123def456",
            "type": "nuban",
            "details": {
                "account_number": "0001234567",
                "account_name": "JOHN DOE",
                "bank_code": "058",
                "bank_name": "GTBank"
            }
        }
    }"#;

    const TRANSFER_RESPONSE: &str = r#"{
        "status": true,
        "message": "Transfer has been queued",
        "data": {
            "transfer_code": "TRF_xyz789ghi012",
            "reference": "aframp-ref-001",
            "status": "success",
            "amount": 500000,
            "currency": "NGN",
            "recipient": "RCP_abc123def456"
        }
    }"#;

    const INSUFFICIENT_BALANCE_RESPONSE: &str = r#"{
        "status": false,
        "message": "Insufficient balance",
        "data": null
    }"#;

    const INVALID_ACCOUNT_RESPONSE: &str = r#"{
        "status": false,
        "message": "Could not resolve account name. You have not enabled your account for transfers",
        "data": null
    }"#;

    fn provider(server: &MockServer) -> PaystackProvider {
        PaystackProvider::with_base_url("sk_test_secret".to_string(), server.uri())
    }

    fn payout_request() -> PayoutRequest {
        PayoutRequest {
            amount: "500000".to_string(),
            account_number: "0001234567".to_string(),
            bank_code: "058".to_string(),
            reference: "aframp-ref-001".to_string(),
        }
    }

    async fn mount_resolve(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/bank/resolve"))
            .and(query_param("account_number", "0001234567"))
            .and(query_param("bank_code", "058"))
            .and(header("authorization", "Bearer sk_test_secret"))
            .respond_with(ResponseTemplate::new(200).set_body_string(RESOLVE_RESPONSE))
            .mount(server)
            .await;
    }

    async fn mount_recipient(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/transferrecipient"))
            .and(body_partial_json(serde_json::json!({
                "type": "nuban",
                "account_number": "0001234567",
                "bank_code": "058",
                "currency": "NGN"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_string(RECIPIENT_RESPONSE))
            .mount(server)
            .await;
    }

    async fn mount_transfer(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/transfer"))
            .and(body_partial_json(serde_json::json!({
                "source": "balance",
                "amount": 500000,
                "recipient": "RCP_abc123def456",
                "reference": "aframp-ref-001"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_string(TRANSFER_RESPONSE))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn resolves_account_against_recorded_response() {
        let server = MockServer::start().await;
        mount_resolve(&server).await;

        let resolved: ResolvedAccount = provider(&server)
            .get(
                "/bank/resolve",
                &[("account_number", "0001234567"), ("bank_code", "058")],
            )
            .await
            .expect("resolve should succeed against recorded fixture");

        assert_eq!(resolved.account_name, "JOHN DOE");
    }

    #[tokio::test]
    async fn creates_recipient_against_recorded_response() {
        let server = MockServer::start().await;
        mount_recipient(&server).await;

        let recipient: Recipient = provider(&server)
            .post(
                "/transferrecipient",
                &serde_json::json!({
                    "type": "nuban",
                    "name": "JOHN DOE",
                    "account_number": "0001234567",
                    "bank_code": "058",
                    "currency": "NGN",
                }),
            )
            .await
            .expect("recipient creation should succeed against recorded fixture");

        assert_eq!(recipient.recipient_code, "RCP_abc123def456");
    }

    #[tokio::test]
    async fn initiates_transfer_against_recorded_response() {
        let server = MockServer::start().await;
        mount_transfer(&server).await;

        let transfer: Transfer = provider(&server)
            .post(
                "/transfer",
                &serde_json::json!({
                    "source": "balance",
                    "amount": 500000,
                    "recipient": "RCP_abc123def456",
                    "reason": "Aframp merchant withdrawal",
                    "reference": "aframp-ref-001",
                }),
            )
            .await
            .expect("transfer should succeed against recorded fixture");

        assert_eq!(transfer.transfer_code, "TRF_xyz789ghi012");
        assert_eq!(transfer.status, "success");
    }

    #[tokio::test]
    async fn create_payout_uses_recorded_responses_end_to_end() {
        let server = MockServer::start().await;
        mount_resolve(&server).await;
        mount_recipient(&server).await;
        mount_transfer(&server).await;

        let result = provider(&server)
            .create_payout(&payout_request())
            .await
            .expect("payout should succeed against recorded fixtures");

        assert_eq!(result.provider, "paystack");
        assert_eq!(result.provider_reference, "TRF_xyz789ghi012");
        assert_eq!(result.status, "success");
    }

    #[tokio::test]
    async fn surfaces_insufficient_balance_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/transfer"))
            .respond_with(ResponseTemplate::new(400).set_body_string(INSUFFICIENT_BALANCE_RESPONSE))
            .mount(&server)
            .await;

        let err = provider(&server)
            .post::<Transfer>("/transfer", &serde_json::json!({}))
            .await
            .expect_err("insufficient balance should surface as an error");

        assert!(err.contains("Insufficient balance"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn surfaces_invalid_account_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/bank/resolve"))
            .respond_with(ResponseTemplate::new(422).set_body_string(INVALID_ACCOUNT_RESPONSE))
            .mount(&server)
            .await;

        let err = provider(&server)
            .get::<ResolvedAccount>(
                "/bank/resolve",
                &[("account_number", "0000000000"), ("bank_code", "058")],
            )
            .await
            .expect_err("invalid account should surface as an error");

        assert!(
            err.contains("Could not resolve account name"),
            "unexpected error: {err}"
        );
    }
}
