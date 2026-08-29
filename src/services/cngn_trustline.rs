//! cNGN Trustline Management for Stellar Network
//!
//! Handles checking, creating, and verifying trustlines for the cNGN stablecoin.
//! A trustline is required for Stellar accounts to hold custom assets like cNGN.

use crate::error::{AppError, AppErrorKind, DomainError, ExternalError};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

/// Stellar base reserve per entry (in XLM)
const BASE_RESERVE_XLM: f64 = 0.5;
const TRUSTLINE_RESERVE_XLM: f64 = 0.5;
const MIN_BALANCE_BUFFER_XLM: f64 = 0.5; // Extra buffer for transaction fees

/// Configuration for cNGN asset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CngnAssetConfig {
    pub asset_code: String,
    pub issuer_public_key: String,
    pub default_limit: Option<String>,
}

impl Default for CngnAssetConfig {
    fn default() -> Self {
        Self {
            asset_code: "cNGN".to_string(),
            issuer_public_key: std::env::var("CNGN_ISSUER_PUBLIC_KEY")
                .unwrap_or_else(|_| "GCNGN_ISSUER_PLACEHOLDER".to_string()),
            default_limit: None, // Unlimited by default
        }
    }
}

impl CngnAssetConfig {
    pub fn from_env() -> Self {
        Self {
            asset_code: std::env::var("CNGN_ASSET_CODE").unwrap_or_else(|_| "cNGN".to_string()),
            issuer_public_key: std::env::var("CNGN_ISSUER_PUBLIC_KEY")
                .unwrap_or_else(|_| "GCNGN_ISSUER_PLACEHOLDER".to_string()),
            default_limit: std::env::var("CNGN_DEFAULT_LIMIT").ok(),
        }
    }
}

/// Result of trustline check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustlineStatus {
    pub exists: bool,
    pub balance: Option<String>,
    pub limit: Option<String>,
    pub is_authorized: bool,
}

/// Trustline transaction details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustlineTransaction {
    pub account_id: String,
    pub asset_code: String,
    pub issuer: String,
    pub limit: Option<String>,
    pub estimated_fee: String,
    pub min_balance_required: String,
}

/// Manager for cNGN trustline operations
pub struct CngnTrustlineService {
    stellar_client: StellarClient,
    cngn_config: CngnAssetConfig,
    verification_timeout: Duration,
    polling_interval: Duration,
}

impl CngnTrustlineService {
    pub fn new(stellar_client: StellarClient) -> Self {
        Self {
            stellar_client,
            cngn_config: CngnAssetConfig::from_env(),
            verification_timeout: Duration::from_secs(30),
            polling_interval: Duration::from_secs(2),
        }
    }

    pub fn with_config(stellar_client: StellarClient, cngn_config: CngnAssetConfig) -> Self {
        Self {
            stellar_client,
            cngn_config,
            verification_timeout: Duration::from_secs(30),
            polling_interval: Duration::from_secs(2),
        }
    }

    /// Check if an account has a trustline for cNGN
    ///
    /// # Arguments
    /// * `account_id` - Stellar account public key
    ///
    /// # Returns
    /// TrustlineStatus with existence, balance, and authorization info
    pub async fn check_trustline(&self, account_id: &str) -> Result<TrustlineStatus, AppError> {
        debug!(
            account_id = %account_id,
            asset_code = %self.cngn_config.asset_code,
            issuer = %self.cngn_config.issuer_public_key,
            "Checking trustline"
        );

        // Fetch account info from Stellar
        let account_info =
            self.stellar_client
                .get_account(account_id)
                .await
                .map_err(|e| match e {
                    StellarError::AccountNotFound { address } => {
                        AppError::new(AppErrorKind::Domain(DomainError::WalletNotFound {
                            wallet_address: address,
                        }))
                    }
                    _ => AppError::from(e),
                })?;

        // Search for cNGN trustline in balances
        let trustline = Self::find_trustline(
            &account_info,
            &self.cngn_config.asset_code,
            &self.cngn_config.issuer_public_key,
        );

        let status = if let Some(balance) = trustline {
            info!(
                account_id = %account_id,
                balance = %balance.balance,
                "Trustline exists"
            );

            TrustlineStatus {
                exists: true,
                balance: Some(balance.balance.clone()),
                limit: balance.limit.clone(),
                is_authorized: balance.is_authorized,
            }
        } else {
            debug!(account_id = %account_id, "Trustline does not exist");

            TrustlineStatus {
                exists: false,
                balance: None,
                limit: None,
                is_authorized: false,
            }
        };

        Ok(status)
    }

    /// Validate that an account has sufficient XLM balance for trustline creation
    ///
    /// # Arguments
    /// * `account_id` - Stellar account public key
    ///
    /// # Returns
    /// Ok(()) if balance is sufficient, Err otherwise with required amount
    pub async fn validate_min_balance(&self, account_id: &str) -> Result<(), AppError> {
        debug!(account_id = %account_id, "Validating minimum balance");

        let account_info = self
            .stellar_client
            .get_account(account_id)
            .await
            .map_err(AppError::from)?;

        // Calculate required reserves
        let num_subentries = account_info.subentry_count as f64;
        let base_reserve = BASE_RESERVE_XLM * 2.0; // Account base reserve
        let subentry_reserve = num_subentries * TRUSTLINE_RESERVE_XLM;
        let new_trustline_reserve = TRUSTLINE_RESERVE_XLM;
        let required_balance =
            base_reserve + subentry_reserve + new_trustline_reserve + MIN_BALANCE_BUFFER_XLM;

        // Get current XLM balance
        let xlm_balance = account_info
            .balances
            .iter()
            .find(|b| b.asset_type == "native")
            .map(|b| b.balance.parse::<f64>().unwrap_or(0.0))
            .unwrap_or(0.0);

        if xlm_balance < required_balance {
            warn!(
                account_id = %account_id,
                current_balance = %xlm_balance,
                required_balance = %required_balance,
                "Insufficient XLM balance for trustline"
            );

            return Err(AppError::new(AppErrorKind::Domain(
                DomainError::InsufficientBalance {
                    available: format!("{:.7} XLM", xlm_balance),
                    required: format!("{:.7} XLM", required_balance),
                },
            )));
        }

        info!(
            account_id = %account_id,
            xlm_balance = %xlm_balance,
            required = %required_balance,
            "Sufficient balance for trustline creation"
        );

        Ok(())
    }

    /// Create a trustline transaction for cNGN
    ///
    /// Note: This prepares the transaction details but does not submit it.
    /// The actual transaction building and submission should be handled by
    /// the transaction builder service.
    ///
    /// # Arguments
    /// * `account_id` - Stellar account public key
    ///
    /// # Returns
    /// TrustlineTransaction with details needed for transaction building
    pub async fn create_trustline_tx(
        &self,
        account_id: &str,
    ) -> Result<TrustlineTransaction, AppError> {
        debug!(
            account_id = %account_id,
            asset_code = %self.cngn_config.asset_code,
            "Creating trustline transaction"
        );

        // First check if trustline already exists
        let status = self.check_trustline(account_id).await?;
        if status.exists {
            warn!(account_id = %account_id, "Trustline already exists");
            crate::metrics::stellar::trustline_attempts_total()
                .with_label_values(&["already_exists"])
                .inc();
            return Err(AppError::new(AppErrorKind::Domain(
                DomainError::DuplicateTransaction {
                    transaction_id: format!("trustline_{}", account_id),
                },
            )));
        }

        // Validate minimum balance
        match self.validate_min_balance(account_id).await {
            Ok(_) => {}
            Err(e) => {
                crate::metrics::stellar::trustline_attempts_total()
                    .with_label_values(&["failed"])
                    .inc();
                return Err(e);
            }
        }

        // Calculate minimum balance requirement
        let account_info = self
            .stellar_client
            .get_account(account_id)
            .await
            .map_err(AppError::from)?;

        let num_subentries = account_info.subentry_count as f64;
        let min_balance_required = BASE_RESERVE_XLM * 2.0
            + (num_subentries + 1.0) * TRUSTLINE_RESERVE_XLM
            + MIN_BALANCE_BUFFER_XLM;

        let tx_details = TrustlineTransaction {
            account_id: account_id.to_string(),
            asset_code: self.cngn_config.asset_code.clone(),
            issuer: self.cngn_config.issuer_public_key.clone(),
            limit: self.cngn_config.default_limit.clone(),
            estimated_fee: "0.00001".to_string(), // Base fee
            min_balance_required: format!("{:.7}", min_balance_required),
        };

        info!(
            account_id = %account_id,
            asset_code = %tx_details.asset_code,
            "Trustline transaction prepared"
        );

        crate::metrics::stellar::trustline_attempts_total()
            .with_label_values(&["success"])
            .inc();

        Ok(tx_details)
    }

    /// Verify that a trustline was successfully created
    ///
    /// Polls Horizon API to confirm the trustline appears in account balances
    ///
    /// # Arguments
    /// * `account_id` - Stellar account public key
    ///
    /// # Returns
    /// Ok(true) if verified, Err if timeout or verification failed
    pub async fn verify_trustline(&self, account_id: &str) -> Result<bool, AppError> {
        info!(
            account_id = %account_id,
            timeout_secs = %self.verification_timeout.as_secs(),
            "Verifying trustline creation"
        );

        let verify_future = async {
            let mut attempts = 0;
            let max_attempts =
                (self.verification_timeout.as_secs() / self.polling_interval.as_secs()) as u32;

            loop {
                attempts += 1;

                // Check if trustline exists
                match self.check_trustline(account_id).await {
                    Ok(status) if status.exists && status.is_authorized => {
                        info!(
                            account_id = %account_id,
                            attempts = %attempts,
                            "Trustline verified successfully"
                        );
                        return Ok(true);
                    }
                    Ok(status) if status.exists && !status.is_authorized => {
                        warn!(
                            account_id = %account_id,
                            "Trustline exists but not authorized"
                        );
                        return Err(AppError::new(AppErrorKind::Domain(
                            DomainError::TrustlineCreationFailed {
                                wallet_address: account_id.to_string(),
                                reason: "Trustline not authorized by issuer".to_string(),
                            },
                        )));
                    }
                    Ok(_) => {
                        // Trustline doesn't exist yet, continue polling
                        if attempts >= max_attempts {
                            break;
                        }
                        sleep(self.polling_interval).await;
                    }
                    Err(e) => {
                        error!(
                            account_id = %account_id,
                            error = ?e,
                            "Error checking trustline during verification"
                        );
                        return Err(e);
                    }
                }
            }

            // Timeout reached
            Err(AppError::new(AppErrorKind::External(
                ExternalError::Timeout {
                    service: "Stellar".to_string(),
                    timeout_secs: self.verification_timeout.as_secs(),
                },
            )))
        };

        timeout(self.verification_timeout, verify_future)
            .await
            .map_err(|_| {
                AppError::new(AppErrorKind::External(ExternalError::Timeout {
                    service: "Stellar".to_string(),
                    timeout_secs: self.verification_timeout.as_secs(),
                }))
            })?
    }

    /// Helper function to find a trustline in account balances
    fn find_trustline<'a>(
        account_info: &'a StellarAccountInfo,
        asset_code: &str,
        issuer: &str,
    ) -> Option<&'a AssetBalance> {
        account_info.balances.iter().find(|balance| {
            balance.asset_type != "native"
                && balance.asset_code.as_deref() == Some(asset_code)
                && balance.asset_issuer.as_deref() == Some(issuer)
        })
    }

    /// Get required XLM balance for trustline creation
    pub fn calculate_required_balance(&self, current_subentries: u32) -> f64 {
        BASE_RESERVE_XLM * 2.0
            + (current_subentries as f64) * TRUSTLINE_RESERVE_XLM
            + TRUSTLINE_RESERVE_XLM
            + MIN_BALANCE_BUFFER_XLM
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cngn_config_default() {
        let config = CngnAssetConfig::default();
        assert_eq!(config.asset_code, "cNGN");
        assert!(config.issuer_public_key.len() > 0);
    }

    #[test]
    fn test_calculate_required_balance() {
        let stellar_client = StellarClient::new(Default::default()).unwrap();
        let manager = CngnTrustlineService::new(stellar_client);

        // Account with 0 subentries
        let balance = manager.calculate_required_balance(0);
        assert_eq!(balance, 2.0); // 1.0 base + 0.5 trustline + 0.5 buffer

        // Account with 2 subentries
        let balance = manager.calculate_required_balance(2);
        assert_eq!(balance, 3.0); // 1.0 base + 1.0 existing + 0.5 trustline + 0.5 buffer
    }

    #[test]
    fn test_trustline_status_creation() {
        let status = TrustlineStatus {
            exists: true,
            balance: Some("100.0000000".to_string()),
            limit: Some("1000000".to_string()),
            is_authorized: true,
        };

        assert!(status.exists);
        assert!(status.is_authorized);
        assert_eq!(status.balance.unwrap(), "100.0000000");
    }
}
