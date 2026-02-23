//! Exchange Rate Service
//!
//! Manages exchange rates between currencies with caching, fee calculation,
//! and historical rate storage. Supports fixed-peg rates (cNGN/NGN) and
//! future external API integration.

use crate::cache::cache::{Cache, RedisCache};
use crate::cache::keys::exchange_rate::CurrencyPairKey;
use crate::database::error::DatabaseError;
use crate::database::exchange_rate_repository::ExchangeRateRepository;
use crate::services::fee_structure::{FeeCalculationInput, FeeStructureService};
use async_trait::async_trait;
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

/// Exchange rate service error
#[derive(Debug, thiserror::Error)]
pub enum ExchangeRateError {
    #[error("Database error: {0}")]
    Database(#[from] DatabaseError),

    #[error("Rate not found for {from} -> {to}")]
    RateNotFound { from: String, to: String },

    #[error("Invalid rate: {0}")]
    InvalidRate(String),

    #[error("Rate provider error: {0}")]
    ProviderError(String),

    #[error("Fee calculation error: {0}")]
    FeeCalculationError(String),

    #[error("Invalid amount: {0}")]
    InvalidAmount(String),
}

pub type ExchangeRateResult<T> = Result<T, ExchangeRateError>;

/// Rate provider trait for fetching exchange rates
#[async_trait]
pub trait RateProvider: Send + Sync {
    /// Fetch current rate between two currencies
    async fn fetch_rate(&self, from: &str, to: &str) -> ExchangeRateResult<RateData>;

    /// Get supported currency pairs
    fn get_supported_pairs(&self) -> Vec<(String, String)>;

    /// Check if provider is healthy
    async fn is_healthy(&self) -> bool;

    /// Get provider name
    fn name(&self) -> &str;
}

/// Rate data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateData {
    pub currency_pair: String,
    pub base_rate: BigDecimal,
    pub buy_rate: BigDecimal,
    pub sell_rate: BigDecimal,
    pub spread: BigDecimal,
    pub source: String,
    pub last_updated: DateTime<Utc>,
}

/// Conversion request
#[derive(Debug, Clone)]
pub struct ConversionRequest {
    pub from_currency: String,
    pub to_currency: String,
    pub amount: BigDecimal,
    pub direction: ConversionDirection,
}

/// Conversion direction (buy or sell)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionDirection {
    Buy,  // Onramp: fiat -> crypto
    Sell, // Offramp: crypto -> fiat
}

/// Conversion result with fee breakdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionResult {
    pub from_currency: String,
    pub to_currency: String,
    pub from_amount: String,
    pub base_rate: String,
    pub gross_amount: String,
    pub fees: FeeBreakdown,
    pub net_amount: String,
    pub expires_at: DateTime<Utc>,
}

/// Fee breakdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeBreakdown {
    pub provider_fee: String,
    pub platform_fee: String,
    pub total_fees: String,
}

/// Exchange rate service configuration
#[derive(Debug, Clone)]
pub struct ExchangeRateServiceConfig {
    pub cache_ttl_seconds: u64,
    pub rate_expiry_seconds: u64,
    pub enable_validation: bool,
    pub max_rate_deviation: BigDecimal, // Maximum allowed deviation from 1.0 for cNGN
}

impl Default for ExchangeRateServiceConfig {
    fn default() -> Self {
        Self {
            cache_ttl_seconds: 60,
            rate_expiry_seconds: 300,
            enable_validation: true,
            max_rate_deviation: BigDecimal::from_str("0.0001").unwrap(),
        }
    }
}

/// Main exchange rate service
pub struct ExchangeRateService {
    repository: ExchangeRateRepository,
    cache: Option<RedisCache>,
    providers: Vec<Arc<dyn RateProvider>>,
    fee_service: Option<Arc<FeeStructureService>>,
    config: ExchangeRateServiceConfig,
}

impl ExchangeRateService {
    /// Create new exchange rate service
    pub fn new(repository: ExchangeRateRepository, config: ExchangeRateServiceConfig) -> Self {
        Self {
            repository,
            cache: None,
            providers: Vec::new(),
            fee_service: None,
            config,
        }
    }

    /// Enable caching
    pub fn with_cache(mut self, cache: RedisCache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Add rate provider
    pub fn add_provider(mut self, provider: Arc<dyn RateProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    /// Set fee service for conversion calculations
    pub fn with_fee_service(mut self, fee_service: Arc<FeeStructureService>) -> Self {
        self.fee_service = Some(fee_service);
        self
    }

    /// Get current exchange rate
    pub async fn get_rate(
        &self,
        from_currency: &str,
        to_currency: &str,
    ) -> ExchangeRateResult<BigDecimal> {
        // Try cache first
        if let Some(cached_rate) = self.get_cached_rate(from_currency, to_currency).await {
            debug!("Cache hit for rate: {} -> {}", from_currency, to_currency);
            return Ok(cached_rate.base_rate);
        }

        // Cache miss - fetch from provider or database
        let rate_data = self.fetch_rate_data(from_currency, to_currency).await?;

        // Cache the result
        if let Some(ref cache) = self.cache {
            let cache_key = CurrencyPairKey::new(from_currency, to_currency);
            let ttl = Duration::from_secs(self.config.cache_ttl_seconds);
            let _ = cache
                .set(&cache_key.to_string(), &rate_data, Some(ttl))
                .await;
        }

        Ok(rate_data.base_rate)
    }

    /// Calculate conversion with fees
    pub async fn calculate_conversion(
        &self,
        request: ConversionRequest,
    ) -> ExchangeRateResult<ConversionResult> {
        // Validate amount
        if request.amount <= BigDecimal::from(0) {
            return Err(ExchangeRateError::InvalidAmount(
                "Amount must be positive".to_string(),
            ));
        }

        // Get exchange rate
        let rate = self
            .get_rate(&request.from_currency, &request.to_currency)
            .await?;

        // Calculate gross amount
        let gross_amount = &request.amount * &rate;

        // Calculate fees
        let fees = self.calculate_fees(&request, &gross_amount).await?;

        // Calculate net amount (gross - fees)
        let net_amount = &gross_amount - &fees.total_fees;

        // Calculate expiry time
        let expires_at =
            Utc::now() + chrono::Duration::seconds(self.config.rate_expiry_seconds as i64);

        Ok(ConversionResult {
            from_currency: request.from_currency.clone(),
            to_currency: request.to_currency.clone(),
            from_amount: request.amount.to_string(),
            base_rate: rate.to_string(),
            gross_amount: gross_amount.to_string(),
            fees: FeeBreakdown {
                provider_fee: fees.provider_fee.to_string(),
                platform_fee: fees.platform_fee.to_string(),
                total_fees: fees.total_fees.to_string(),
            },
            net_amount: net_amount.to_string(),
            expires_at,
        })
    }

    /// Get historical rate at specific timestamp
    pub async fn get_historical_rate(
        &self,
        from_currency: &str,
        to_currency: &str,
        _timestamp: DateTime<Utc>,
    ) -> ExchangeRateResult<BigDecimal> {
        // For now, get the most recent rate
        // TODO: Implement time-based query
        let rate = self
            .repository
            .get_current_rate(from_currency, to_currency)
            .await?
            .ok_or_else(|| ExchangeRateError::RateNotFound {
                from: from_currency.to_string(),
                to: to_currency.to_string(),
            })?;

        BigDecimal::from_str(&rate.rate).map_err(|e| ExchangeRateError::InvalidRate(e.to_string()))
    }

    /// Update exchange rate
    pub async fn update_rate(
        &self,
        from_currency: &str,
        to_currency: &str,
        rate: BigDecimal,
        source: &str,
    ) -> ExchangeRateResult<()> {
        // Validate rate
        if self.config.enable_validation {
            self.validate_rate(from_currency, to_currency, &rate)?;
        }

        // Store in database
        self.repository
            .upsert_rate(from_currency, to_currency, &rate.to_string(), Some(source))
            .await?;

        // Invalidate cache
        if let Some(ref cache) = self.cache {
            let cache_key = CurrencyPairKey::new(from_currency, to_currency);
            let _ = <RedisCache as Cache<RateData>>::delete(cache, &cache_key.to_string()).await;
        }

        debug!(
            "Updated rate: {} -> {} = {} (source: {})",
            from_currency, to_currency, rate, source
        );

        Ok(())
    }

    /// Invalidate cached rate
    pub async fn invalidate_cache(
        &self,
        from_currency: &str,
        to_currency: &str,
    ) -> ExchangeRateResult<()> {
        if let Some(ref cache) = self.cache {
            let cache_key = CurrencyPairKey::new(from_currency, to_currency);
            <RedisCache as Cache<RateData>>::delete(cache, &cache_key.to_string())
                .await
                .ok();
        }
        Ok(())
    }

    // Private helper methods

    async fn get_cached_rate(&self, from_currency: &str, to_currency: &str) -> Option<RateData> {
        if let Some(ref cache) = self.cache {
            let cache_key = CurrencyPairKey::new(from_currency, to_currency);
            cache.get(&cache_key.to_string()).await.ok().flatten()
        } else {
            None
        }
    }

    async fn fetch_rate_data(
        &self,
        from_currency: &str,
        to_currency: &str,
    ) -> ExchangeRateResult<RateData> {
        // Try providers first
        for provider in &self.providers {
            if provider.is_healthy().await {
                match provider.fetch_rate(from_currency, to_currency).await {
                    Ok(rate_data) => {
                        // Store in database for historical record
                        let _ = self
                            .repository
                            .upsert_rate(
                                from_currency,
                                to_currency,
                                &rate_data.base_rate.to_string(),
                                Some(&rate_data.source),
                            )
                            .await;
                        return Ok(rate_data);
                    }
                    Err(e) => {
                        warn!("Provider {} failed to fetch rate: {}", provider.name(), e);
                        continue;
                    }
                }
            }
        }

        // Fallback to database
        let rate = self
            .repository
            .get_current_rate(from_currency, to_currency)
            .await?
            .ok_or_else(|| ExchangeRateError::RateNotFound {
                from: from_currency.to_string(),
                to: to_currency.to_string(),
            })?;

        let base_rate = BigDecimal::from_str(&rate.rate)
            .map_err(|e| ExchangeRateError::InvalidRate(e.to_string()))?;

        Ok(RateData {
            currency_pair: format!("{}/{}", from_currency, to_currency),
            base_rate: base_rate.clone(),
            buy_rate: base_rate.clone(),
            sell_rate: base_rate.clone(),
            spread: BigDecimal::from(0),
            source: rate.source.unwrap_or_else(|| "database".to_string()),
            last_updated: rate.updated_at,
        })
    }

    async fn calculate_fees(
        &self,
        request: &ConversionRequest,
        gross_amount: &BigDecimal,
    ) -> ExchangeRateResult<FeeCalculation> {
        let fee_service = match &self.fee_service {
            Some(service) => service,
            None => {
                // No fee service configured, return zero fees
                return Ok(FeeCalculation {
                    provider_fee: BigDecimal::from(0),
                    platform_fee: BigDecimal::from(0),
                    total_fees: BigDecimal::from(0),
                });
            }
        };

        // Calculate provider fee (1.4%)
        let provider_fee_input = FeeCalculationInput {
            fee_type: "provider_fee".to_string(),
            amount: gross_amount.clone(),
            currency: Some(request.to_currency.clone()),
            at_time: None,
        };

        let provider_fee = match fee_service.calculate_fee(provider_fee_input).await {
            Ok(Some(result)) => result.fee,
            Ok(None) => BigDecimal::from(0),
            Err(e) => {
                warn!("Failed to calculate provider fee: {}", e);
                BigDecimal::from(0)
            }
        };

        // Calculate platform fee (0.1%)
        let platform_fee_input = FeeCalculationInput {
            fee_type: "platform_fee".to_string(),
            amount: gross_amount.clone(),
            currency: Some(request.to_currency.clone()),
            at_time: None,
        };

        let platform_fee = match fee_service.calculate_fee(platform_fee_input).await {
            Ok(Some(result)) => result.fee,
            Ok(None) => BigDecimal::from(0),
            Err(e) => {
                warn!("Failed to calculate platform fee: {}", e);
                BigDecimal::from(0)
            }
        };

        let total_fees = &provider_fee + &platform_fee;

        Ok(FeeCalculation {
            provider_fee,
            platform_fee,
            total_fees,
        })
    }

    fn validate_rate(
        &self,
        from_currency: &str,
        to_currency: &str,
        rate: &BigDecimal,
    ) -> ExchangeRateResult<()> {
        // For cNGN/NGN, rate should always be 1.0 (±0.0001 tolerance)
        if (from_currency == "NGN" && to_currency == "cNGN")
            || (from_currency == "cNGN" && to_currency == "NGN")
        {
            let one = BigDecimal::from(1);
            let deviation = (rate - &one).abs();

            if deviation > self.config.max_rate_deviation {
                return Err(ExchangeRateError::InvalidRate(format!(
                    "cNGN/NGN rate must be 1.0 (±{}), got {}",
                    self.config.max_rate_deviation, rate
                )));
            }
        }

        // Rate must be positive
        if rate <= &BigDecimal::from(0) {
            return Err(ExchangeRateError::InvalidRate(
                "Rate must be positive".to_string(),
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
struct FeeCalculation {
    provider_fee: BigDecimal,
    platform_fee: BigDecimal,
    total_fees: BigDecimal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversion_direction() {
        assert_eq!(ConversionDirection::Buy, ConversionDirection::Buy);
        assert_ne!(ConversionDirection::Buy, ConversionDirection::Sell);
    }

    #[tokio::test]
    async fn test_rate_validation() {
        let config = ExchangeRateServiceConfig::default();
        let repo = ExchangeRateRepository::new(
            sqlx::PgPool::connect_lazy("postgresql://localhost/test").unwrap(),
        );
        let service = ExchangeRateService::new(repo, config);

        // Valid cNGN rate
        let result = service.validate_rate("NGN", "cNGN", &BigDecimal::from(1));
        assert!(result.is_ok());

        // Invalid cNGN rate (too far from 1.0)
        let result = service.validate_rate("NGN", "cNGN", &BigDecimal::from_str("1.5").unwrap());
        assert!(result.is_err());

        // Negative rate
        let result = service.validate_rate("USD", "NGN", &BigDecimal::from(-1));
        assert!(result.is_err());
    }
}
