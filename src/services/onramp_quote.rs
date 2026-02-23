//! Onramp Quote Service
//!
//! Handles NGN → cNGN quote creation: rate snapshot, fee calculation,
//! liquidity check, trustline verification, and Redis storage.

use crate::cache::cache::Cache;
use crate::cache::keys::onramp::QuoteKey;
use crate::cache::RedisCache;
use crate::chains::stellar::client::StellarClient;
use crate::chains::stellar::trustline::CngnTrustlineManager;
use crate::chains::stellar::types::{extract_cngn_balance, is_valid_stellar_address};
use crate::error::{AppError, AppErrorKind, DomainError, ValidationError};
use crate::services::exchange_rate::{ConversionDirection, ConversionRequest, ExchangeRateService};
use crate::services::fee_structure::{FeeCalculationInput, FeeStructureService};
use bigdecimal::{BigDecimal, Zero};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};
use uuid::Uuid;

/// Minimum onramp amount in NGN (₦1,000)
const MIN_ONRAMP_AMOUNT_NGN: i64 = 1000;

/// Quote TTL in seconds (3 minutes)
const QUOTE_TTL_SECS: u64 = 180;

/// Provider enum for onramp
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PaymentProvider {
    Flutterwave,
    Paystack,
    Other(String),
}

impl PaymentProvider {
    pub fn as_str(&self) -> &str {
        match self {
            PaymentProvider::Flutterwave => "flutterwave",
            PaymentProvider::Paystack => "paystack",
            PaymentProvider::Other(s) => s.as_str(),
        }
    }
}

impl From<&str> for PaymentProvider {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "flutterwave" => PaymentProvider::Flutterwave,
            "paystack" => PaymentProvider::Paystack,
            _ => PaymentProvider::Other(s.to_string()),
        }
    }
}

/// Chain enum
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Chain {
    Stellar,
    Other(String),
}

impl Chain {
    pub fn as_str(&self) -> &str {
        match self {
            Chain::Stellar => "stellar",
            Chain::Other(s) => s.as_str(),
        }
    }
}

impl From<&str> for Chain {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "stellar" => Chain::Stellar,
            _ => Chain::Other(s.to_string()),
        }
    }
}

fn decimal_to_i64_trunc(value: &BigDecimal) -> i64 {
    value
        .to_string()
        .split('.')
        .next()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
}

/// API request for onramp quote
#[derive(Debug, Clone, Deserialize)]
pub struct OnrampQuoteRequest {
    pub amount_ngn: i64,
    pub wallet_address: String,
    pub provider: String,
    pub chain: Option<String>,
}

/// Stored quote data in Redis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredQuote {
    pub quote_id: String,
    pub wallet_address: String,
    pub amount_ngn: i64,
    pub amount_cngn: String,
    pub rate_snapshot: String,
    pub platform_fee_ngn: String,
    pub provider_fee_ngn: String,
    pub total_fee_ngn: String,
    pub provider: String,
    pub chain: String,
    pub created_at: String,
    pub expires_at: String,
    pub status: String,
}

/// API response for onramp quote
#[derive(Debug, Clone, Serialize)]
pub struct OnrampQuoteResponse {
    pub quote_id: String,
    pub expires_at: String,
    pub expires_in_seconds: u64,
    pub input: QuoteInput,
    pub fees: QuoteFees,
    pub output: QuoteOutput,
    pub trustline_required: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuoteInput {
    pub amount_ngn: i64,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuoteFees {
    pub platform_fee_ngn: i64,
    pub provider_fee_ngn: i64,
    pub total_fee_ngn: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuoteOutput {
    pub amount_ngn_after_fees: i64,
    pub rate: f64,
    pub amount_cngn: i64,
    pub chain: String,
}

pub struct OnrampQuoteService {
    exchange_rate_service: Arc<ExchangeRateService>,
    fee_service: Arc<FeeStructureService>,
    stellar_client: StellarClient,
    redis_cache: RedisCache,
    cngn_issuer: String,
    liquidity_check_enabled: bool,
}

impl OnrampQuoteService {
    pub fn new(
        exchange_rate_service: Arc<ExchangeRateService>,
        fee_service: Arc<FeeStructureService>,
        stellar_client: StellarClient,
        redis_cache: RedisCache,
        cngn_issuer: String,
    ) -> Self {
        let liquidity_check_enabled = std::env::var("ONRAMP_LIQUIDITY_CHECK")
            .unwrap_or_else(|_| "true".to_string())
            .to_lowercase()
            != "false";

        Self {
            exchange_rate_service,
            fee_service,
            stellar_client,
            redis_cache,
            cngn_issuer,
            liquidity_check_enabled,
        }
    }

    /// Create an onramp quote
    pub async fn create_quote(
        &self,
        request: OnrampQuoteRequest,
    ) -> Result<OnrampQuoteResponse, AppError> {
        // 1. Validate wallet address
        let wallet_address = request.wallet_address.trim();
        if wallet_address.is_empty() {
            return Err(AppError::new(AppErrorKind::Validation(
                ValidationError::MissingField {
                    field: "wallet_address".to_string(),
                },
            )));
        }
        if !is_valid_stellar_address(wallet_address) {
            return Err(AppError::new(AppErrorKind::Validation(
                ValidationError::InvalidWalletAddress {
                    address: wallet_address.to_string(),
                    reason: "Stellar wallet address is invalid or does not exist".to_string(),
                },
            )));
        }

        // 2. Validate amount
        if request.amount_ngn < MIN_ONRAMP_AMOUNT_NGN {
            return Err(AppError::new(AppErrorKind::Domain(
                DomainError::AmountTooLow {
                    amount: request.amount_ngn.to_string(),
                    minimum: MIN_ONRAMP_AMOUNT_NGN.to_string(),
                },
            )));
        }

        let amount_bd = BigDecimal::from(request.amount_ngn);
        let requested_chain = request
            .chain
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .unwrap_or("stellar");
        let chain = Chain::from(requested_chain).as_str().to_string();
        let provider = request.provider.trim();
        if provider.is_empty() {
            return Err(AppError::new(AppErrorKind::Validation(
                ValidationError::MissingField {
                    field: "provider".to_string(),
                },
            )));
        }
        let provider = PaymentProvider::from(provider).as_str().to_string();

        // 3. Fetch cached rate and calculate conversion
        let conversion = self
            .exchange_rate_service
            .calculate_conversion(ConversionRequest {
                from_currency: "NGN".to_string(),
                to_currency: "cNGN".to_string(),
                amount: amount_bd.clone(),
                direction: ConversionDirection::Buy,
            })
            .await
            .map_err(|e| {
                AppError::new(AppErrorKind::External(
                    crate::error::ExternalError::Blockchain {
                        message: e.to_string(),
                        is_retryable: true,
                    },
                ))
            })?;

        // Parse fees from conversion result
        let platform_fee_ngn = BigDecimal::from_str(&conversion.fees.platform_fee)
            .unwrap_or_else(|_| BigDecimal::from(0));
        let provider_fee_ngn = BigDecimal::from_str(&conversion.fees.provider_fee)
            .unwrap_or_else(|_| BigDecimal::from(0));
        // If fee service returned zeros, try onramp-specific fee types
        let (platform_fee_ngn, provider_fee_ngn) =
            if platform_fee_ngn.is_zero() && provider_fee_ngn.is_zero() {
                self.calculate_onramp_fees(&amount_bd).await?
            } else {
                (platform_fee_ngn, provider_fee_ngn)
            };

        let total_fee_ngn = &platform_fee_ngn + &provider_fee_ngn;
        let amount_ngn_after_fees = &amount_bd - &total_fee_ngn;
        if amount_ngn_after_fees <= BigDecimal::from(0) {
            return Err(AppError::new(AppErrorKind::Domain(
                DomainError::InvalidAmount {
                    amount: request.amount_ngn.to_string(),
                    reason: "Amount after fees must be greater than zero".to_string(),
                },
            )));
        }

        // Use conversion net amount when available; fall back to NGN-after-fees for peg scenarios.
        let amount_cngn_bd = BigDecimal::from_str(&conversion.net_amount)
            .ok()
            .filter(|v| v > &BigDecimal::from(0))
            .unwrap_or_else(|| amount_ngn_after_fees.clone());
        let rate =
            BigDecimal::from_str(&conversion.base_rate).unwrap_or_else(|_| BigDecimal::from(1));

        // 4. Check cNGN liquidity
        if self.liquidity_check_enabled {
            self.check_liquidity(&amount_cngn_bd).await?;
        }

        // 5. Check trustline
        let trustline_manager = CngnTrustlineManager::new(self.stellar_client.clone());
        let trustline_status = trustline_manager
            .check_trustline(wallet_address)
            .await
            .map_err(|e| match e {
                crate::chains::stellar::errors::StellarError::InvalidAddress { .. } => {
                    AppError::new(AppErrorKind::Validation(
                        ValidationError::InvalidWalletAddress {
                            address: wallet_address.to_string(),
                            reason: "Stellar wallet address is invalid or does not exist"
                                .to_string(),
                        },
                    ))
                }
                _ => AppError::from(e),
            })?;
        let trustline_required = !trustline_status.has_trustline;

        // 6. Generate quote_id and persist to Redis
        let quote_id = format!("q_{}", Uuid::new_v4().simple());
        let expires_at = Utc::now() + chrono::Duration::seconds(QUOTE_TTL_SECS as i64);

        let stored = StoredQuote {
            quote_id: quote_id.clone(),
            wallet_address: wallet_address.to_string(),
            amount_ngn: request.amount_ngn,
            amount_cngn: amount_cngn_bd.to_string(),
            rate_snapshot: conversion.base_rate.clone(),
            platform_fee_ngn: platform_fee_ngn.to_string(),
            provider_fee_ngn: provider_fee_ngn.to_string(),
            total_fee_ngn: total_fee_ngn.to_string(),
            provider: provider.clone(),
            chain: chain.clone(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: expires_at.to_rfc3339(),
            status: "pending".to_string(),
        };

        let cache_key = QuoteKey::new(&quote_id).to_string();
        self.redis_cache
            .set(
                &cache_key,
                &stored,
                Some(Duration::from_secs(QUOTE_TTL_SECS)),
            )
            .await
            .map_err(|e| {
                AppError::new(AppErrorKind::Infrastructure(
                    crate::error::InfrastructureError::Cache {
                        message: format!("Failed to store quote: {}", e),
                    },
                ))
            })?;

        debug!(quote_id = %quote_id, "Stored quote in Redis");

        let amount_cngn_int = decimal_to_i64_trunc(&amount_cngn_bd);
        let amount_ngn_after_fees_int = decimal_to_i64_trunc(&amount_ngn_after_fees);

        Ok(OnrampQuoteResponse {
            quote_id,
            expires_at: expires_at.to_rfc3339(),
            expires_in_seconds: QUOTE_TTL_SECS,
            input: QuoteInput {
                amount_ngn: request.amount_ngn,
                provider,
            },
            fees: QuoteFees {
                platform_fee_ngn: decimal_to_i64_trunc(&platform_fee_ngn),
                provider_fee_ngn: decimal_to_i64_trunc(&provider_fee_ngn),
                total_fee_ngn: decimal_to_i64_trunc(&total_fee_ngn),
            },
            output: QuoteOutput {
                amount_ngn_after_fees: amount_ngn_after_fees_int,
                rate: rate.to_string().parse().unwrap_or(1.0),
                amount_cngn: amount_cngn_int,
                chain,
            },
            trustline_required,
        })
    }

    async fn calculate_onramp_fees(
        &self,
        amount_ngn: &BigDecimal,
    ) -> Result<(BigDecimal, BigDecimal), AppError> {
        let platform_fee = self
            .fee_service
            .calculate_fee(FeeCalculationInput {
                fee_type: "onramp_platform".to_string(),
                amount: amount_ngn.clone(),
                currency: Some("NGN".to_string()),
                at_time: None,
            })
            .await
            .map_err(|e| {
                AppError::new(AppErrorKind::Infrastructure(
                    crate::error::InfrastructureError::Database {
                        message: e.to_string(),
                        is_retryable: true,
                    },
                ))
            })?;

        let provider_fee = self
            .fee_service
            .calculate_fee(FeeCalculationInput {
                fee_type: "onramp_provider".to_string(),
                amount: amount_ngn.clone(),
                currency: Some("NGN".to_string()),
                at_time: None,
            })
            .await
            .map_err(|e| {
                AppError::new(AppErrorKind::Infrastructure(
                    crate::error::InfrastructureError::Database {
                        message: e.to_string(),
                        is_retryable: true,
                    },
                ))
            })?;

        let platform_fee_bd = platform_fee
            .map(|r| r.fee)
            .unwrap_or_else(|| BigDecimal::from(0));
        let provider_fee_bd = provider_fee
            .map(|r| r.fee)
            .unwrap_or_else(|| BigDecimal::from(0));

        if platform_fee_bd.is_zero() && provider_fee_bd.is_zero() {
            let total = self
                .fee_service
                .calculate_fee(FeeCalculationInput {
                    fee_type: "onramp".to_string(),
                    amount: amount_ngn.clone(),
                    currency: Some("NGN".to_string()),
                    at_time: None,
                })
                .await
                .map_err(|e| {
                    AppError::new(AppErrorKind::Infrastructure(
                        crate::error::InfrastructureError::Database {
                            message: e.to_string(),
                            is_retryable: true,
                        },
                    ))
                })?;

            let total_fee = total.map(|r| r.fee).unwrap_or_else(|| BigDecimal::from(0));
            let platform = &total_fee * BigDecimal::from_str("0.2").unwrap();
            let provider = &total_fee - &platform;
            return Ok((platform, provider));
        }

        Ok((platform_fee_bd, provider_fee_bd))
    }

    async fn check_liquidity(&self, amount_cngn: &BigDecimal) -> Result<(), AppError> {
        let distribution_account = std::env::var("CNGN_DISTRIBUTION_ACCOUNT")
            .or_else(|_| std::env::var("CNGN_ISSUER_ADDRESS"))
            .or_else(|_| std::env::var("CNGN_ISSUER_MAINNET"))
            .unwrap_or_else(|_| self.cngn_issuer.clone());

        let account = self
            .stellar_client
            .get_account(&distribution_account)
            .await
            .map_err(|e| {
                info!(
                    "Liquidity check skipped: could not fetch distribution account: {}",
                    e
                );
                AppError::new(AppErrorKind::External(
                    crate::error::ExternalError::Blockchain {
                        message: e.to_string(),
                        is_retryable: true,
                    },
                ))
            })?;

        let available = extract_cngn_balance(&account.balances, Some(&self.cngn_issuer));
        let available_bd: BigDecimal = available
            .and_then(|s| BigDecimal::from_str(&s).ok())
            .unwrap_or_else(|| BigDecimal::from(0));

        if available_bd < *amount_cngn {
            return Err(AppError::new(AppErrorKind::Domain(
                DomainError::InsufficientLiquidity {
                    amount: amount_cngn.to_string(),
                },
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_min_onramp_amount() {
        assert_eq!(MIN_ONRAMP_AMOUNT_NGN, 1000);
    }

    #[test]
    fn test_quote_ttl() {
        assert_eq!(QUOTE_TTL_SECS, 180);
    }

    #[test]
    fn test_payment_provider_from_str() {
        assert_eq!(PaymentProvider::from("flutterwave").as_str(), "flutterwave");
        assert_eq!(PaymentProvider::from("paystack").as_str(), "paystack");
        assert_eq!(PaymentProvider::from("other").as_str(), "other");
    }

    #[test]
    fn test_chain_from_str() {
        assert_eq!(Chain::from("stellar").as_str(), "stellar");
    }

    #[test]
    fn test_amount_too_low_validation() {
        let request = OnrampQuoteRequest {
            amount_ngn: 500,
            wallet_address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            provider: "flutterwave".to_string(),
            chain: Some("stellar".to_string()),
        };
        assert!(request.amount_ngn < MIN_ONRAMP_AMOUNT_NGN);
    }

    #[test]
    fn test_valid_amount() {
        let request = OnrampQuoteRequest {
            amount_ngn: 50000,
            wallet_address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            provider: "flutterwave".to_string(),
            chain: Some("stellar".to_string()),
        };
        assert!(request.amount_ngn >= MIN_ONRAMP_AMOUNT_NGN);
    }
}
