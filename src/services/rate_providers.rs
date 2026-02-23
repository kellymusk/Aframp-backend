//! Rate providers for fetching exchange rates
//!
//! Implements different rate providers:
//! - FixedRateProvider: For cNGN 1:1 peg with NGN
//! - ExternalApiProvider: For future external API integration

use super::exchange_rate::{ExchangeRateError, ExchangeRateResult, RateData, RateProvider};
use async_trait::async_trait;
use bigdecimal::BigDecimal;
use chrono::Utc;
#[cfg(test)]
use std::str::FromStr;
use tracing::debug;

/// Fixed rate provider for cNGN/NGN 1:1 peg
pub struct FixedRateProvider {
    supported_pairs: Vec<(String, String)>,
}

impl FixedRateProvider {
    pub fn new() -> Self {
        Self {
            supported_pairs: vec![
                ("NGN".to_string(), "cNGN".to_string()),
                ("cNGN".to_string(), "NGN".to_string()),
            ],
        }
    }
}

impl Default for FixedRateProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RateProvider for FixedRateProvider {
    async fn fetch_rate(&self, from: &str, to: &str) -> ExchangeRateResult<RateData> {
        // Check if this is a supported pair
        let is_supported = self
            .supported_pairs
            .iter()
            .any(|(f, t)| f == from && t == to);

        if !is_supported {
            return Err(ExchangeRateError::RateNotFound {
                from: from.to_string(),
                to: to.to_string(),
            });
        }

        // Return fixed 1:1 rate
        let one = BigDecimal::from(1);

        Ok(RateData {
            currency_pair: format!("{}/{}", from, to),
            base_rate: one.clone(),
            buy_rate: one.clone(),
            sell_rate: one.clone(),
            spread: BigDecimal::from(0),
            source: "fixed_peg".to_string(),
            last_updated: Utc::now(),
        })
    }

    fn get_supported_pairs(&self) -> Vec<(String, String)> {
        self.supported_pairs.clone()
    }

    async fn is_healthy(&self) -> bool {
        true // Always healthy since it's a fixed rate
    }

    fn name(&self) -> &str {
        "FixedRateProvider"
    }
}

/// External API provider for fetching rates from external sources
/// This is a placeholder for future implementation with CoinGecko, Fixer.io, etc.
pub struct ExternalApiProvider {
    api_url: String,
    api_key: Option<String>,
    supported_pairs: Vec<(String, String)>,
    timeout_seconds: u64,
}

impl ExternalApiProvider {
    pub fn new(api_url: String, api_key: Option<String>) -> Self {
        Self {
            api_url,
            api_key,
            supported_pairs: Vec::new(),
            timeout_seconds: 10,
        }
    }

    pub fn with_timeout(mut self, timeout_seconds: u64) -> Self {
        self.timeout_seconds = timeout_seconds;
        self
    }

    pub fn add_supported_pair(mut self, from: String, to: String) -> Self {
        self.supported_pairs.push((from, to));
        self
    }
}

#[async_trait]
impl RateProvider for ExternalApiProvider {
    async fn fetch_rate(&self, from: &str, to: &str) -> ExchangeRateResult<RateData> {
        // Check if this is a supported pair
        let is_supported = self
            .supported_pairs
            .iter()
            .any(|(f, t)| f == from && t == to);

        if !is_supported {
            return Err(ExchangeRateError::RateNotFound {
                from: from.to_string(),
                to: to.to_string(),
            });
        }

        // TODO: Implement actual API call to external service
        // For now, return an error indicating it's not implemented
        debug!(
            "ExternalApiProvider: Would fetch rate from {} for {} -> {}",
            self.api_url, from, to
        );

        Err(ExchangeRateError::ProviderError(
            "External API provider not yet implemented".to_string(),
        ))
    }

    fn get_supported_pairs(&self) -> Vec<(String, String)> {
        self.supported_pairs.clone()
    }

    async fn is_healthy(&self) -> bool {
        // TODO: Implement health check by pinging the API
        // For now, return false since it's not implemented
        false
    }

    fn name(&self) -> &str {
        "ExternalApiProvider"
    }
}

/// Multi-source rate provider that aggregates rates from multiple sources
pub struct AggregatedRateProvider {
    providers: Vec<Box<dyn RateProvider>>,
    aggregation_strategy: AggregationStrategy,
}

#[derive(Debug, Clone, Copy)]
pub enum AggregationStrategy {
    Average,
    Median,
    First,
}

impl AggregatedRateProvider {
    pub fn new(strategy: AggregationStrategy) -> Self {
        Self {
            providers: Vec::new(),
            aggregation_strategy: strategy,
        }
    }

    pub fn add_provider(mut self, provider: Box<dyn RateProvider>) -> Self {
        self.providers.push(provider);
        self
    }
}

#[async_trait]
impl RateProvider for AggregatedRateProvider {
    async fn fetch_rate(&self, from: &str, to: &str) -> ExchangeRateResult<RateData> {
        if self.providers.is_empty() {
            return Err(ExchangeRateError::ProviderError(
                "No providers configured".to_string(),
            ));
        }

        let mut rates = Vec::new();
        let mut last_error = None;

        // Fetch rates from all providers
        for provider in &self.providers {
            if provider.is_healthy().await {
                match provider.fetch_rate(from, to).await {
                    Ok(rate_data) => rates.push(rate_data),
                    Err(e) => {
                        last_error = Some(e);
                        continue;
                    }
                }
            }
        }

        if rates.is_empty() {
            return Err(last_error.unwrap_or_else(|| {
                ExchangeRateError::ProviderError("All providers failed".to_string())
            }));
        }

        // Aggregate rates based on strategy
        let aggregated_rate = match self.aggregation_strategy {
            AggregationStrategy::First => rates[0].base_rate.clone(),
            AggregationStrategy::Average => {
                let sum: BigDecimal = rates.iter().map(|r| &r.base_rate).sum();
                sum / BigDecimal::from(rates.len() as u64)
            }
            AggregationStrategy::Median => {
                let mut sorted_rates: Vec<BigDecimal> =
                    rates.iter().map(|r| r.base_rate.clone()).collect();
                sorted_rates.sort();
                let mid = sorted_rates.len() / 2;
                if sorted_rates.len() % 2 == 0 {
                    (&sorted_rates[mid - 1] + &sorted_rates[mid]) / BigDecimal::from(2)
                } else {
                    sorted_rates[mid].clone()
                }
            }
        };

        Ok(RateData {
            currency_pair: format!("{}/{}", from, to),
            base_rate: aggregated_rate.clone(),
            buy_rate: aggregated_rate.clone(),
            sell_rate: aggregated_rate.clone(),
            spread: BigDecimal::from(0),
            source: format!("aggregated_{:?}", self.aggregation_strategy),
            last_updated: Utc::now(),
        })
    }

    fn get_supported_pairs(&self) -> Vec<(String, String)> {
        // Return union of all supported pairs
        let mut pairs = Vec::new();
        for provider in &self.providers {
            for pair in provider.get_supported_pairs() {
                if !pairs.contains(&pair) {
                    pairs.push(pair);
                }
            }
        }
        pairs
    }

    async fn is_healthy(&self) -> bool {
        // At least one provider must be healthy
        for provider in &self.providers {
            if provider.is_healthy().await {
                return true;
            }
        }
        false
    }

    fn name(&self) -> &str {
        "AggregatedRateProvider"
    }
}

/// Mock rate provider for testing
#[cfg(test)]
pub struct MockRateProvider {
    rate: BigDecimal,
    healthy: bool,
}

#[cfg(test)]
impl MockRateProvider {
    pub fn new(rate: f64) -> Self {
        Self {
            rate: BigDecimal::from_str(&rate.to_string()).unwrap(),
            healthy: true,
        }
    }

    pub fn with_health(mut self, healthy: bool) -> Self {
        self.healthy = healthy;
        self
    }
}

#[cfg(test)]
#[async_trait]
impl RateProvider for MockRateProvider {
    async fn fetch_rate(&self, from: &str, to: &str) -> ExchangeRateResult<RateData> {
        Ok(RateData {
            currency_pair: format!("{}/{}", from, to),
            base_rate: self.rate.clone(),
            buy_rate: self.rate.clone(),
            sell_rate: self.rate.clone(),
            spread: BigDecimal::from(0),
            source: "mock".to_string(),
            last_updated: Utc::now(),
        })
    }

    fn get_supported_pairs(&self) -> Vec<(String, String)> {
        vec![("USD".to_string(), "NGN".to_string())]
    }

    async fn is_healthy(&self) -> bool {
        self.healthy
    }

    fn name(&self) -> &str {
        "MockRateProvider"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fixed_rate_provider() {
        let provider = FixedRateProvider::new();

        // Test NGN -> cNGN
        let rate = provider.fetch_rate("NGN", "cNGN").await.unwrap();
        assert_eq!(rate.base_rate, BigDecimal::from(1));
        assert_eq!(rate.source, "fixed_peg");

        // Test cNGN -> NGN
        let rate = provider.fetch_rate("cNGN", "NGN").await.unwrap();
        assert_eq!(rate.base_rate, BigDecimal::from(1));

        // Test unsupported pair
        let result = provider.fetch_rate("USD", "NGN").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_fixed_rate_provider_health() {
        let provider = FixedRateProvider::new();
        assert!(provider.is_healthy().await);
    }

    #[tokio::test]
    async fn test_aggregated_provider_average() {
        let provider1 = Box::new(MockRateProvider::new(1500.0));
        let provider2 = Box::new(MockRateProvider::new(1600.0));

        let aggregated = AggregatedRateProvider::new(AggregationStrategy::Average)
            .add_provider(provider1)
            .add_provider(provider2);

        let rate = aggregated.fetch_rate("USD", "NGN").await.unwrap();
        let expected = BigDecimal::from_str("1550").unwrap();
        assert_eq!(rate.base_rate, expected);
    }

    #[tokio::test]
    async fn test_aggregated_provider_median() {
        let provider1 = Box::new(MockRateProvider::new(1500.0));
        let provider2 = Box::new(MockRateProvider::new(1600.0));
        let provider3 = Box::new(MockRateProvider::new(1700.0));

        let aggregated = AggregatedRateProvider::new(AggregationStrategy::Median)
            .add_provider(provider1)
            .add_provider(provider2)
            .add_provider(provider3);

        let rate = aggregated.fetch_rate("USD", "NGN").await.unwrap();
        let expected = BigDecimal::from_str("1600").unwrap();
        assert_eq!(rate.base_rate, expected);
    }
}
