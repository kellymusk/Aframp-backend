use crate::chains::stellar::{
    config::StellarConfig,
    errors::{StellarError, StellarResult},
    types::{
        extract_afri_balance, is_valid_stellar_address, HealthStatus, HorizonAccount,
        StellarAccountInfo,
    },
};
use reqwest::Client;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

#[allow(dead_code)]
pub struct StellarClient {
    http_client: Client,
    config: StellarConfig,
}

#[allow(dead_code)]
impl StellarClient {
    pub fn new(config: StellarConfig) -> StellarResult<Self> {
        config
            .validate()
            .map_err(|e| StellarError::config_error(e.to_string()))?;

        let http_client = Client::builder()
            .timeout(config.request_timeout)
            .user_agent("Aframp-Backend/1.0")
            .build()
            .map_err(|e| {
                StellarError::config_error(format!("Failed to create HTTP client: {}", e))
            })?;

        info!(
            "Stellar client initialized for {:?} network with URL: {}",
            config.network,
            config.network.horizon_url()
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

        let url = format!("{}/accounts/{}", self.config.network.horizon_url(), address);

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

    pub async fn health_check(&self) -> StellarResult<HealthStatus> {
        let start_time = Instant::now();
        let horizon_url = self.config.network.horizon_url();

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
}
