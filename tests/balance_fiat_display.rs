/// Issue #1112 — multi-currency balance display with NGN fiat equivalent
///
/// These are specification/acceptance tests for the `?include_fiat=true`
/// feature described in `docs/proposals/1112-multi-currency-balance-fiat.md`.
///
/// Because the protected `src/` hasn't been updated yet, this file defines
/// local mirrors of the proposed types and logic so the tests:
///
/// 1. Compile and run today against the spec (no database required)
/// 2. Document exactly what the implementation must do
/// 3. Act as acceptance criteria the implementer can run against the real code
///    once `src/` is updated
///
/// When the src/ implementation lands, replace the local mirror block with:
/// ```rust
/// use aframp::services::exchange_rates::{ExchangeRate, ExchangeRateProvider};
/// use aframp::services::exchange_rates_mock::MockExchangeRateProvider;
/// use aframp::models::BalanceWithFiat;
/// ```

// ---------------------------------------------------------------------------
// Local mirror of the proposed types (remove once src/ is updated)
// ---------------------------------------------------------------------------

mod fiat_iface {
    use async_trait::async_trait;
    use std::collections::HashMap;

    /// Mirrors `src/services/exchange_rates.rs` (to be created).
    #[derive(Debug, Clone)]
    pub struct ExchangeRate {
        pub asset: String,
        /// How many NGN one whole unit of `asset` is worth.
        /// e.g. XLM = 1245.50 means 1 XLM = ₦1,245.50
        pub ngn_per_unit: f64,
        pub source: String,
        pub fetched_at: chrono::DateTime<chrono::Utc>,
    }

    /// Mirrors `src/services/exchange_rates.rs` (to be created).
    #[async_trait]
    pub trait ExchangeRateProvider: Send + Sync {
        async fn get_rates(
            &self,
            assets: &[String],
        ) -> Result<HashMap<String, ExchangeRate>, String>;
    }

    /// Mirrors `src/services/exchange_rates_mock.rs` (to be created).
    pub struct MockExchangeRateProvider {
        pub rates: HashMap<String, f64>,
    }

    #[async_trait]
    impl ExchangeRateProvider for MockExchangeRateProvider {
        async fn get_rates(
            &self,
            assets: &[String],
        ) -> Result<HashMap<String, ExchangeRate>, String> {
            let now = chrono::Utc::now();
            let result = assets
                .iter()
                .filter_map(|asset| {
                    self.rates.get(asset.as_str()).map(|&rate| {
                        (
                            asset.clone(),
                            ExchangeRate {
                                asset: asset.clone(),
                                ngn_per_unit: rate,
                                source: "mock".to_string(),
                                fetched_at: now,
                            },
                        )
                    })
                })
                .collect();
            Ok(result)
        }
    }

    /// A mock that always fails.
    pub struct FailingExchangeRateProvider;

    #[async_trait]
    impl ExchangeRateProvider for FailingExchangeRateProvider {
        async fn get_rates(
            &self,
            _assets: &[String],
        ) -> Result<HashMap<String, ExchangeRate>, String> {
            Err("exchange rate service temporarily unavailable".to_string())
        }
    }

    // -----------------------------------------------------------------------
    // Balance types mirroring the proposed src/models/balance.rs additions
    // -----------------------------------------------------------------------

    #[derive(Debug, Clone, PartialEq)]
    pub struct Balance {
        pub asset: String,
        pub available: i64, // stroops
        pub pending: i64,   // stroops
    }

    #[derive(Debug, Clone, PartialEq)]
    pub struct FiatEquivalent {
        pub available_ngn: f64,
        pub pending_ngn: f64,
        pub rate_per_unit: f64,
        pub source: String,
    }

    #[derive(Debug, Clone)]
    pub struct BalanceWithFiat {
        pub balance: Balance,
        pub fiat: Option<FiatEquivalent>,
    }

    // -----------------------------------------------------------------------
    // Service function mirroring the proposed balance enrichment logic
    // -----------------------------------------------------------------------

    const STROOPS_PER_UNIT: f64 = 10_000_000.0;

    /// Enriches a list of balances with NGN fiat equivalents.
    /// If the provider fails, the balances are returned without fiat data
    /// (graceful degradation — no 500).
    pub async fn enrich_with_fiat(
        balances: Vec<Balance>,
        provider: &dyn ExchangeRateProvider,
    ) -> Vec<BalanceWithFiat> {
        let assets: Vec<String> = balances.iter().map(|b| b.asset.clone()).collect();

        let rates = match provider.get_rates(&assets).await {
            Ok(r) => r,
            Err(err) => {
                // Log and degrade gracefully.
                eprintln!("exchange rate fetch failed: {err}");
                std::collections::HashMap::new()
            }
        };

        balances
            .into_iter()
            .map(|b| {
                let fiat = rates.get(&b.asset).map(|rate| FiatEquivalent {
                    available_ngn: (b.available as f64 / STROOPS_PER_UNIT) * rate.ngn_per_unit,
                    pending_ngn: (b.pending as f64 / STROOPS_PER_UNIT) * rate.ngn_per_unit,
                    rate_per_unit: rate.ngn_per_unit,
                    source: rate.source.clone(),
                });
                BalanceWithFiat { balance: b, fiat }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod balance_fiat_tests {
    use std::collections::HashMap;

    use super::fiat_iface::*;

    fn xlm_balance(available: i64, pending: i64) -> Balance {
        Balance {
            asset: "XLM".to_string(),
            available,
            pending,
        }
    }

    fn cngn_balance(available: i64, pending: i64) -> Balance {
        Balance {
            asset: "cNGN".to_string(),
            available,
            pending,
        }
    }

    // -----------------------------------------------------------------------
    // MockExchangeRateProvider basics
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn mock_provider_returns_configured_rates() {
        let provider = MockExchangeRateProvider {
            rates: HashMap::from([("XLM".to_string(), 1245.50)]),
        };

        let rates = provider
            .get_rates(&["XLM".to_string()])
            .await
            .expect("mock should succeed");

        assert!(rates.contains_key("XLM"));
        assert!(
            (rates["XLM"].ngn_per_unit - 1245.50).abs() < 0.01,
            "rate should be 1245.50"
        );
        assert_eq!(rates["XLM"].source, "mock");
    }

    #[tokio::test]
    async fn mock_provider_returns_empty_for_unknown_asset() {
        let provider = MockExchangeRateProvider {
            rates: HashMap::from([("XLM".to_string(), 1245.50)]),
        };

        let rates = provider
            .get_rates(&["UNKNOWN_ASSET".to_string()])
            .await
            .expect("mock should succeed");

        assert!(
            rates.is_empty(),
            "unknown asset should not appear in result"
        );
    }

    #[tokio::test]
    async fn failing_provider_returns_error() {
        let provider = FailingExchangeRateProvider;
        let result = provider.get_rates(&["XLM".to_string()]).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // enrich_with_fiat — happy path
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn xlm_balance_enriched_with_correct_ngn_equivalent() {
        // 1 XLM = 10_000_000 stroops; rate = ₦1,245.50/XLM
        let provider = MockExchangeRateProvider {
            rates: HashMap::from([("XLM".to_string(), 1245.50)]),
        };

        let balances = vec![xlm_balance(10_000_000, 5_000_000)];
        let result = enrich_with_fiat(balances, &provider).await;

        assert_eq!(result.len(), 1);
        let fiat = result[0].fiat.as_ref().expect("should have fiat equivalent");

        // 1 XLM available → ₦1,245.50
        assert!(
            (fiat.available_ngn - 1245.50).abs() < 0.01,
            "available_ngn should be ₦1,245.50 for 1 XLM"
        );
        // 0.5 XLM pending → ₦622.75
        assert!(
            (fiat.pending_ngn - 622.75).abs() < 0.01,
            "pending_ngn should be ₦622.75 for 0.5 XLM"
        );
        assert!(
            (fiat.rate_per_unit - 1245.50).abs() < 0.01,
            "rate_per_unit should be ₦1,245.50"
        );
        assert_eq!(fiat.source, "mock");
    }

    #[tokio::test]
    async fn cngn_balance_enriched_with_one_to_one_rate() {
        // cNGN is pegged 1:1 to NGN
        let provider = MockExchangeRateProvider {
            rates: HashMap::from([("cNGN".to_string(), 1.0)]),
        };

        // 14_500 cNGN available (14_500 * 10_000_000 stroops)
        let balances = vec![cngn_balance(145_000_000_000, 0)];
        let result = enrich_with_fiat(balances, &provider).await;

        let fiat = result[0].fiat.as_ref().expect("should have fiat");
        assert!(
            (fiat.available_ngn - 14500.0).abs() < 0.01,
            "14,500 cNGN at 1:1 should be ₦14,500"
        );
        assert!((fiat.pending_ngn - 0.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn multiple_assets_each_enriched_independently() {
        let provider = MockExchangeRateProvider {
            rates: HashMap::from([
                ("XLM".to_string(), 1245.50),
                ("cNGN".to_string(), 1.0),
            ]),
        };

        let balances = vec![
            xlm_balance(10_000_000, 0),   // 1 XLM
            cngn_balance(10_000_000, 0),  // 1 cNGN
        ];
        let result = enrich_with_fiat(balances, &provider).await;

        assert_eq!(result.len(), 2);

        let xlm_fiat = result[0].fiat.as_ref().expect("XLM should have fiat");
        assert!((xlm_fiat.available_ngn - 1245.50).abs() < 0.01);

        let cngn_fiat = result[1].fiat.as_ref().expect("cNGN should have fiat");
        assert!((cngn_fiat.available_ngn - 1.0).abs() < 0.01);
    }

    // -----------------------------------------------------------------------
    // enrich_with_fiat — graceful degradation
    // -----------------------------------------------------------------------

    /// When the exchange rate provider fails, the function MUST NOT panic or
    /// return an error — it should return balances with `fiat: None`.
    #[tokio::test]
    async fn provider_failure_degrades_gracefully_no_panic() {
        let provider = FailingExchangeRateProvider;
        let balances = vec![xlm_balance(10_000_000, 0)];
        let result = enrich_with_fiat(balances, &provider).await;

        assert_eq!(result.len(), 1, "balance list should still be returned");
        assert!(
            result[0].fiat.is_none(),
            "fiat should be None when provider fails — not a 500"
        );
    }

    /// When the rate for a specific asset is missing from the provider response
    /// (provider succeeded but didn't include that asset), fiat should be None
    /// for that asset only — not a hard failure.
    #[tokio::test]
    async fn missing_rate_for_asset_produces_none_fiat() {
        let provider = MockExchangeRateProvider {
            rates: HashMap::from([("XLM".to_string(), 1245.50)]),
            // cNGN not in the provider's rate map
        };

        let balances = vec![
            xlm_balance(10_000_000, 0),
            cngn_balance(10_000_000, 0),
        ];
        let result = enrich_with_fiat(balances, &provider).await;

        // XLM should have fiat
        assert!(result[0].fiat.is_some(), "XLM should have fiat when rate is available");
        // cNGN should not have fiat (rate missing)
        assert!(
            result[1].fiat.is_none(),
            "cNGN should have no fiat when rate is not in provider response"
        );
    }

    // -----------------------------------------------------------------------
    // Zero balances
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn zero_balance_produces_zero_ngn_equivalent() {
        let provider = MockExchangeRateProvider {
            rates: HashMap::from([("XLM".to_string(), 1245.50)]),
        };

        let balances = vec![xlm_balance(0, 0)];
        let result = enrich_with_fiat(balances, &provider).await;

        let fiat = result[0].fiat.as_ref().expect("should have fiat even for zero balance");
        assert!((fiat.available_ngn - 0.0).abs() < 0.01);
        assert!((fiat.pending_ngn - 0.0).abs() < 0.01);
    }

    // -----------------------------------------------------------------------
    // No balances at all
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn empty_balance_list_returns_empty_result() {
        let provider = MockExchangeRateProvider {
            rates: HashMap::new(),
        };

        let result = enrich_with_fiat(vec![], &provider).await;
        assert!(result.is_empty());
    }

    // -----------------------------------------------------------------------
    // Trait object usage
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn provider_usable_as_dyn_trait_object() {
        let provider: Box<dyn ExchangeRateProvider> = Box::new(MockExchangeRateProvider {
            rates: HashMap::from([("XLM".to_string(), 1245.50)]),
        });

        let rates = provider
            .get_rates(&["XLM".to_string()])
            .await
            .expect("should succeed");
        assert!(rates.contains_key("XLM"));
    }
}
