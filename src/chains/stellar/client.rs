use crate::chains::stellar::{
    config::StellarConfig,
    errors::{StellarError, StellarResult},
    types::{
        extract_afri_balance, extract_asset_balance, extract_cngn_balance,
        is_valid_stellar_address, HealthStatus, HorizonAccount, StellarAccountInfo,
    },
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct StellarClient {
    http_client: Client,
    config: StellarConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HorizonTransactionRecord {
    pub id: Option<String>,
    pub paging_token: Option<String>,
    pub hash: String,
    #[serde(default)]
    pub successful: bool,
    pub ledger: Option<i64>,
    pub created_at: Option<String>,
    pub memo_type: Option<String>,
    pub memo: Option<String>,
    pub result_xdr: Option<String>,
    pub result_meta_xdr: Option<String>,
    pub envelope_xdr: Option<String>,
    pub fee_charged: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HorizonTransactionsPage {
    pub records: Vec<HorizonTransactionRecord>,
}

#[allow(dead_code)]
impl StellarClient {
    pub fn new(config: StellarConfig) -> StellarResult<Self> {
        config
            .validate()
            .map_err(|e| StellarError::config_error(e.to_string()))?;

        let http_client = Client::builder()
            .timeout(config.request_timeout)
            .pool_max_idle_per_host(20)
            .user_agent("Aframp-Backend/1.0")
            .build()
            .map_err(|e| {
                StellarError::config_error(format!("Failed to create HTTP client: {}", e))
            })?;

        info!(
            "Stellar client initialized for {:?} network with URL: {}",
            config.network,
            config.horizon_url()
        );

        Ok(Self {
            http_client,
            config,
        })
    }

    pub async fn get_account(&self, address: &str) -> StellarResult<StellarAccountInfo> {
        if !is_valid_stellar_address(address) {
            return Err(StellarError::invalid_address(address));
        }

        debug!("Fetching account details for address: {}", address);

        let url = format!("{}/accounts/{}", self.config.horizon_url(), address);

        let response = timeout(
            self.config.request_timeout,
            self.http_client.get(&url).send(),
        )
        .await
        .map_err(|_| StellarError::timeout_error(self.config.request_timeout.as_secs()))?;

        let response = response.map_err(|e| {
            if e.status() == Some(reqwest::StatusCode::NOT_FOUND) {
                StellarError::account_not_found(address)
            } else if e.status() == Some(reqwest::StatusCode::TOO_MANY_REQUESTS) {
                StellarError::RateLimitError
            } else {
                StellarError::network_error(format!("Horizon API error: {}", e))
            }
        })?;

        let response = response.error_for_status().map_err(|e: reqwest::Error| {
            if e.status() == Some(reqwest::StatusCode::NOT_FOUND) {
                StellarError::account_not_found(address)
            } else if e.status() == Some(reqwest::StatusCode::TOO_MANY_REQUESTS) {
                StellarError::RateLimitError
            } else {
                StellarError::network_error(format!("Horizon API error: {}", e))
            }
        })?;

        let account_result: HorizonAccount = response
            .json()
            .await
            .map_err(|e| StellarError::network_error(format!("JSON parsing error: {}", e)))?;

        let account_info = StellarAccountInfo::from(account_result);

        debug!("Successfully fetched account for address: {}", address);
        Ok(account_info)
    }

    pub async fn account_exists(&self, address: &str) -> StellarResult<bool> {
        if !is_valid_stellar_address(address) {
            return Err(StellarError::invalid_address(address));
        }

        debug!("Checking if account exists: {}", address);

        match self.get_account(address).await {
            Ok(_) => {
                debug!("Account exists: {}", address);
                Ok(true)
            }
            Err(StellarError::AccountNotFound { .. }) => {
                debug!("Account does not exist: {}", address);
                Ok(false)
            }
            Err(e) => {
                warn!("Error checking account existence for {}: {}", address, e);
                Err(e)
            }
        }
    }

    pub async fn get_balances(&self, address: &str) -> StellarResult<Vec<String>> {
        let account = self.get_account(address).await?;
        let balances: Vec<String> = account
            .balances
            .iter()
            .map(|balance| match balance.asset_type.as_str() {
                "native" => format!("XLM: {}", balance.balance),
                "credit_alphanum4" | "credit_alphanum12" => {
                    format!(
                        "{}:{}:{}",
                        balance.asset_code.as_deref().unwrap_or("UNKNOWN"),
                        balance.asset_issuer.as_deref().unwrap_or("UNKNOWN"),
                        balance.balance
                    )
                }
                _ => format!("{}:{}", balance.asset_type, balance.balance),
            })
            .collect();

        debug!(
            "Retrieved {} balances for address: {}",
            balances.len(),
            address
        );
        Ok(balances)
    }

    pub async fn get_afri_balance(&self, address: &str) -> StellarResult<Option<String>> {
        let account = self.get_account(address).await?;
        let afri_balance = extract_afri_balance(&account.balances);

        debug!(
            "AFRI balance for address {}: {}",
            address,
            afri_balance.as_deref().unwrap_or("None")
        );

        Ok(afri_balance)
    }

    pub async fn get_cngn_balance(
        &self,
        address: &str,
        issuer: Option<&str>,
    ) -> StellarResult<Option<String>> {
        let account = self.get_account(address).await?;
        let cngn_balance = extract_cngn_balance(&account.balances, issuer);

        debug!(
            "cNGN balance for address {}: {}",
            address,
            cngn_balance.as_deref().unwrap_or("None")
        );

        Ok(cngn_balance)
    }

    pub async fn get_asset_balance(
        &self,
        address: &str,
        asset_code: &str,
        issuer: Option<&str>,
    ) -> StellarResult<Option<String>> {
        let account = self.get_account(address).await?;
        Ok(extract_asset_balance(&account.balances, asset_code, issuer))
    }

    pub async fn health_check(&self) -> StellarResult<HealthStatus> {
        let start_time = Instant::now();
        let horizon_url = self.config.horizon_url();

        debug!(
            "Performing health check for Stellar Horizon at: {}",
            horizon_url
        );

        // Use config timeout for health check (default 10s, but allow longer for slow networks)
        let health_timeout = std::cmp::max(self.config.request_timeout, Duration::from_secs(15));

        let result = timeout(
            health_timeout,
            self.http_client.get(format!("{}/", horizon_url)).send(),
        )
        .await;

        let response_time = start_time.elapsed();

        match result {
            Ok(Ok(response)) if response.status().is_success() => {
                info!(
                    "Stellar Horizon health check passed - Response time: {}ms",
                    response_time.as_millis()
                );

                Ok(HealthStatus {
                    is_healthy: true,
                    horizon_url: horizon_url.to_string(),
                    response_time_ms: response_time.as_millis() as u64,
                    last_check: chrono::Utc::now().to_rfc3339(),
                    error_message: None,
                })
            }
            Ok(Ok(response)) => {
                let error_msg = format!("HTTP status: {}", response.status());
                error!("Stellar Horizon health check failed: {}", error_msg);

                Ok(HealthStatus {
                    is_healthy: false,
                    horizon_url: horizon_url.to_string(),
                    response_time_ms: response_time.as_millis() as u64,
                    last_check: chrono::Utc::now().to_rfc3339(),
                    error_message: Some(error_msg),
                })
            }
            Ok(Err(e)) => {
                let error_msg = format!("Request failed: {}", e);
                error!("Stellar Horizon health check failed: {}", error_msg);

                Ok(HealthStatus {
                    is_healthy: false,
                    horizon_url: horizon_url.to_string(),
                    response_time_ms: response_time.as_millis() as u64,
                    last_check: chrono::Utc::now().to_rfc3339(),
                    error_message: Some(error_msg),
                })
            }
            Err(_) => {
                let error_msg = format!(
                    "Request timed out after {} seconds",
                    health_timeout.as_secs()
                );
                error!("Stellar Horizon health check failed: {}", error_msg);

                Ok(HealthStatus {
                    is_healthy: false,
                    horizon_url: horizon_url.to_string(),
                    response_time_ms: response_time.as_millis() as u64,
                    last_check: chrono::Utc::now().to_rfc3339(),
                    error_message: Some(error_msg),
                })
            }
        }
    }

    pub fn config(&self) -> &StellarConfig {
        &self.config
    }

    pub fn network(&self) -> &crate::chains::stellar::config::StellarNetwork {
        &self.config.network
    }

    pub async fn submit_transaction_xdr(&self, xdr_base64: &str) -> StellarResult<JsonValue> {
        let url = format!("{}/transactions", self.config.horizon_url());

        let response = timeout(
            self.config.request_timeout,
            self.http_client
                .post(&url)
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(format!("tx={}", encode_form_component(xdr_base64)))
                .send(),
        )
        .await
        .map_err(|_| StellarError::timeout_error(self.config.request_timeout.as_secs()))?;

        let response = response.map_err(|e| {
            if e.status() == Some(reqwest::StatusCode::TOO_MANY_REQUESTS) {
                StellarError::RateLimitError
            } else {
                StellarError::network_error(format!("Horizon submit error: {}", e))
            }
        })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            StellarError::network_error(format!("Horizon submit read error: {}", e))
        })?;

        if !status.is_success() {
            return Err(StellarError::transaction_failed(format!(
                "Horizon submit failed (status {}): {}",
                status, body
            )));
        }

        let json = serde_json::from_str::<JsonValue>(&body).map_err(|e| {
            StellarError::serialization_error(format!("Horizon submit JSON parse error: {}", e))
        })?;

        Ok(json)
    }

    pub async fn get_transaction_by_hash(
        &self,
        tx_hash: &str,
    ) -> StellarResult<HorizonTransactionRecord> {
        let url = format!("{}/transactions/{}", self.config.horizon_url(), tx_hash);
        let response = timeout(
            self.config.request_timeout,
            self.http_client.get(&url).send(),
        )
        .await
        .map_err(|_| StellarError::timeout_error(self.config.request_timeout.as_secs()))?;

        let response = response.map_err(|e| {
            if e.status() == Some(reqwest::StatusCode::NOT_FOUND) {
                StellarError::transaction_failed(format!("transaction not found: {}", tx_hash))
            } else if e.status() == Some(reqwest::StatusCode::TOO_MANY_REQUESTS) {
                StellarError::RateLimitError
            } else {
                StellarError::network_error(format!("Horizon transaction fetch error: {}", e))
            }
        })?;

        let response = response.error_for_status().map_err(|e| {
            if e.status() == Some(reqwest::StatusCode::NOT_FOUND) {
                StellarError::transaction_failed(format!("transaction not found: {}", tx_hash))
            } else if e.status() == Some(reqwest::StatusCode::TOO_MANY_REQUESTS) {
                StellarError::RateLimitError
            } else {
                StellarError::network_error(format!("Horizon transaction fetch error: {}", e))
            }
        })?;

        response
            .json::<HorizonTransactionRecord>()
            .await
            .map_err(|e| StellarError::serialization_error(format!("JSON parsing error: {}", e)))
    }

    pub async fn list_account_transactions(
        &self,
        account: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> StellarResult<HorizonTransactionsPage> {
        if !is_valid_stellar_address(account) {
            return Err(StellarError::invalid_address(account));
        }

        let mut url = format!(
            "{}/accounts/{}/transactions?order=asc&limit={}",
            self.config.horizon_url(),
            account,
            limit.min(200)
        );
        if let Some(c) = cursor {
            url.push_str("&cursor=");
            url.push_str(&encode_form_component(c));
        }

        let response = timeout(
            self.config.request_timeout,
            self.http_client.get(&url).send(),
        )
        .await
        .map_err(|_| StellarError::timeout_error(self.config.request_timeout.as_secs()))?
        .map_err(|e| {
            if e.status() == Some(reqwest::StatusCode::TOO_MANY_REQUESTS) {
                StellarError::RateLimitError
            } else {
                StellarError::network_error(format!("Horizon account tx listing error: {}", e))
            }
        })?
        .error_for_status()
        .map_err(|e| {
            if e.status() == Some(reqwest::StatusCode::TOO_MANY_REQUESTS) {
                StellarError::RateLimitError
            } else {
                StellarError::network_error(format!("Horizon account tx listing error: {}", e))
            }
        })?;

        let body = response
            .json::<JsonValue>()
            .await
            .map_err(|e| StellarError::serialization_error(format!("JSON parsing error: {}", e)))?;

        let records = body
            .get("_embedded")
            .and_then(|v| v.get("records"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|record| serde_json::from_value::<HorizonTransactionRecord>(record).ok())
            .collect::<Vec<_>>();

        Ok(HorizonTransactionsPage { records })
    }

    pub async fn get_transaction_operations(&self, tx_hash: &str) -> StellarResult<Vec<JsonValue>> {
        let response = timeout(
            self.config.request_timeout,
            self.http_client
                .get(format!(
                    "{}/transactions/{}/operations?limit=200",
                    self.config.horizon_url(),
                    tx_hash
                ))
                .send(),
        )
        .await
        .map_err(|_| StellarError::timeout_error(self.config.request_timeout.as_secs()))?
        .map_err(|e| {
            if e.status() == Some(reqwest::StatusCode::TOO_MANY_REQUESTS) {
                StellarError::RateLimitError
            } else {
                StellarError::network_error(format!("Horizon operations fetch error: {}", e))
            }
        })?
        .error_for_status()
        .map_err(|e| {
            if e.status() == Some(reqwest::StatusCode::TOO_MANY_REQUESTS) {
                StellarError::RateLimitError
            } else {
                StellarError::network_error(format!("Horizon operations fetch error: {}", e))
            }
        })?;

        let body = response
            .json::<JsonValue>()
            .await
            .map_err(|e| StellarError::serialization_error(format!("JSON parsing error: {}", e)))?;

        Ok(body
            .get("_embedded")
            .and_then(|v| v.get("records"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }
}

fn encode_form_component(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            output.push(char::from(b));
        } else {
            output.push('%');
            output.push(hex_char((b >> 4) & 0x0F));
            output.push(hex_char(b & 0x0F));
        }
    }
    output
}

fn hex_char(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        10..=15 => (b'A' + nibble - 10) as char,
        _ => '0',
    }
}
