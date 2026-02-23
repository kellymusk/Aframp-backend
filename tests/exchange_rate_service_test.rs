//! Integration tests for Exchange Rate Service
//!
//! Tests the complete exchange rate service including:
//! - Rate fetching and caching
//! - Conversion calculations with fees
//! - Historical rate storage
//! - Rate validation

#[cfg(all(test, feature = "database", feature = "cache"))]
mod tests {
    use bigdecimal::BigDecimal;
    use chrono::Utc;
    use sqlx::PgPool;
    use std::str::FromStr;
    use std::sync::Arc;
    use uuid::Uuid;
    use Bitmesh_backend::cache::cache::{Cache, RedisCache};
    use Bitmesh_backend::cache::init_cache_pool;
    use Bitmesh_backend::cache::CacheConfig;
    use Bitmesh_backend::database::exchange_rate_repository::ExchangeRateRepository;
    use Bitmesh_backend::database::fee_structure_repository::{
        FeeStructure, FeeStructureRepository,
    };
    use Bitmesh_backend::database::repository::Repository;
    use Bitmesh_backend::services::exchange_rate::{
        ConversionDirection, ConversionRequest, ExchangeRateService, ExchangeRateServiceConfig,
    };
    use Bitmesh_backend::services::fee_structure::FeeStructureService;
    use Bitmesh_backend::services::rate_providers::FixedRateProvider;

    async fn setup_test_db() -> PgPool {
        let database_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://localhost/aframp_test".to_string());

        PgPool::connect(&database_url)
            .await
            .expect("Failed to connect to test database")
    }

    async fn setup_test_cache() -> RedisCache {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());

        let config = CacheConfig {
            redis_url,
            max_connections: 5,
            min_idle: 1,
            ..CacheConfig::default()
        };

        let pool = init_cache_pool(config)
            .await
            .expect("Failed to initialize cache pool");

        RedisCache::new(pool)
    }

    async fn setup_fee_structures(pool: &PgPool) {
        // Insert test fee structures
        let provider_fee = FeeStructure {
            id: Uuid::new_v4(),
            fee_type: "provider_fee".to_string(),
            fee_rate_bps: 140, // 1.4%
            fee_flat: BigDecimal::from(0),
            min_fee: None,
            max_fee: None,
            currency: Some("NGN".to_string()),
            is_active: true,
            effective_from: Utc::now() - chrono::Duration::days(1),
            effective_until: None,
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let platform_fee = FeeStructure {
            id: Uuid::new_v4(),
            fee_type: "platform_fee".to_string(),
            fee_rate_bps: 10, // 0.1%
            fee_flat: BigDecimal::from(0),
            min_fee: None,
            max_fee: None,
            currency: Some("NGN".to_string()),
            is_active: true,
            effective_from: Utc::now() - chrono::Duration::days(1),
            effective_until: None,
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let repo = FeeStructureRepository::new(pool.clone());
        let _ = repo.insert(&provider_fee).await;
        let _ = repo.insert(&platform_fee).await;
    }

    #[tokio::test]
    #[ignore] // Requires database and Redis
    async fn test_get_rate_cngn_ngn() {
        let pool = setup_test_db().await;
        let cache = setup_test_cache().await;

        let repo = ExchangeRateRepository::with_cache(pool.clone(), cache.clone());
        let provider = Arc::new(FixedRateProvider::new());

        let service = ExchangeRateService::new(repo, ExchangeRateServiceConfig::default())
            .with_cache(cache)
            .add_provider(provider);

        // Test NGN -> cNGN rate
        let rate = service.get_rate("NGN", "cNGN").await.unwrap();
        assert_eq!(rate, BigDecimal::from(1));

        // Test cNGN -> NGN rate
        let rate = service.get_rate("cNGN", "NGN").await.unwrap();
        assert_eq!(rate, BigDecimal::from(1));
    }

    #[tokio::test]
    #[ignore] // Requires database and Redis
    async fn test_rate_caching() {
        let pool = setup_test_db().await;
        let cache = setup_test_cache().await;

        let repo = ExchangeRateRepository::with_cache(pool.clone(), cache.clone());
        let provider = Arc::new(FixedRateProvider::new());

        let service = ExchangeRateService::new(repo, ExchangeRateServiceConfig::default())
            .with_cache(cache.clone())
            .add_provider(provider);

        // First call - cache miss
        let rate1 = service.get_rate("NGN", "cNGN").await.unwrap();

        // Second call - should hit cache
        let rate2 = service.get_rate("NGN", "cNGN").await.unwrap();

        assert_eq!(rate1, rate2);
        assert_eq!(rate1, BigDecimal::from(1));
    }

    #[tokio::test]
    #[ignore] // Requires database and Redis
    async fn test_conversion_calculation_with_fees() {
        let pool = setup_test_db().await;
        let cache = setup_test_cache().await;

        // Setup fee structures
        setup_fee_structures(&pool).await;

        let repo = ExchangeRateRepository::with_cache(pool.clone(), cache.clone());
        let provider = Arc::new(FixedRateProvider::new());

        let fee_repo = FeeStructureRepository::new(pool.clone());
        let fee_service = Arc::new(FeeStructureService::new(fee_repo));

        let service = ExchangeRateService::new(repo, ExchangeRateServiceConfig::default())
            .with_cache(cache)
            .add_provider(provider)
            .with_fee_service(fee_service);

        // Test conversion: 50,000 NGN -> cNGN
        let request = ConversionRequest {
            from_currency: "NGN".to_string(),
            to_currency: "cNGN".to_string(),
            amount: BigDecimal::from(50000),
            direction: ConversionDirection::Buy,
        };

        let result = service.calculate_conversion(request).await.unwrap();

        // Verify base rate
        assert_eq!(result.base_rate, "1");

        // Verify gross amount (50,000 * 1.0)
        assert_eq!(result.gross_amount, "50000");

        // Verify fees are calculated
        // Provider fee: 1.4% of 50,000 = 700
        // Platform fee: 0.1% of 50,000 = 50
        // Total fees: 750
        let total_fees = BigDecimal::from_str(&result.fees.total_fees).unwrap();
        assert!(total_fees > BigDecimal::from(0));

        // Net amount should be less than gross
        let net_amount = BigDecimal::from_str(&result.net_amount).unwrap();
        let gross_amount = BigDecimal::from_str(&result.gross_amount).unwrap();
        assert!(net_amount < gross_amount);
    }

    #[tokio::test]
    #[ignore] // Requires database
    async fn test_update_rate() {
        let pool = setup_test_db().await;
        let cache = setup_test_cache().await;

        let repo = ExchangeRateRepository::with_cache(pool.clone(), cache.clone());
        let service =
            ExchangeRateService::new(repo, ExchangeRateServiceConfig::default()).with_cache(cache);

        // Update rate
        let new_rate = BigDecimal::from(1);
        service
            .update_rate("NGN", "cNGN", new_rate.clone(), "test")
            .await
            .unwrap();

        // Verify rate was stored
        let stored_rate = service.get_rate("NGN", "cNGN").await.unwrap();
        assert_eq!(stored_rate, new_rate);
    }

    #[tokio::test]
    #[ignore] // Requires database
    async fn test_rate_validation() {
        let pool = setup_test_db().await;
        let repo = ExchangeRateRepository::new(pool.clone());

        let service = ExchangeRateService::new(repo, ExchangeRateServiceConfig::default());

        // Test invalid cNGN rate (too far from 1.0)
        let invalid_rate = BigDecimal::from_str("1.5").unwrap();
        let result = service
            .update_rate("NGN", "cNGN", invalid_rate, "test")
            .await;
        assert!(result.is_err());

        // Test negative rate
        let negative_rate = BigDecimal::from(-1);
        let result = service
            .update_rate("USD", "NGN", negative_rate, "test")
            .await;
        assert!(result.is_err());

        // Test valid rate
        let valid_rate = BigDecimal::from(1);
        let result = service.update_rate("NGN", "cNGN", valid_rate, "test").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[ignore] // Requires database
    async fn test_invalid_amount_conversion() {
        let pool = setup_test_db().await;
        let repo = ExchangeRateRepository::new(pool.clone());
        let provider = Arc::new(FixedRateProvider::new());

        let service = ExchangeRateService::new(repo, ExchangeRateServiceConfig::default())
            .add_provider(provider);

        // Test negative amount
        let request = ConversionRequest {
            from_currency: "NGN".to_string(),
            to_currency: "cNGN".to_string(),
            amount: BigDecimal::from(-100),
            direction: ConversionDirection::Buy,
        };

        let result = service.calculate_conversion(request).await;
        assert!(result.is_err());

        // Test zero amount
        let request = ConversionRequest {
            from_currency: "NGN".to_string(),
            to_currency: "cNGN".to_string(),
            amount: BigDecimal::from(0),
            direction: ConversionDirection::Buy,
        };

        let result = service.calculate_conversion(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    #[ignore] // Requires database and Redis
    async fn test_cache_invalidation() {
        let pool = setup_test_db().await;
        let cache = setup_test_cache().await;

        let repo = ExchangeRateRepository::with_cache(pool.clone(), cache.clone());
        let provider = Arc::new(FixedRateProvider::new());

        let service = ExchangeRateService::new(repo, ExchangeRateServiceConfig::default())
            .with_cache(cache.clone())
            .add_provider(provider);

        // Get rate to populate cache
        let _ = service.get_rate("NGN", "cNGN").await.unwrap();

        // Invalidate cache
        service.invalidate_cache("NGN", "cNGN").await.unwrap();

        // Verify cache was cleared
        let cache_key =
            Bitmesh_backend::cache::keys::exchange_rate::CurrencyPairKey::new("NGN", "cNGN");
        let cached: Option<Bitmesh_backend::services::exchange_rate::RateData> =
            cache.get(&cache_key.to_string()).await.unwrap();
        assert!(cached.is_none());
    }

    #[tokio::test]
    #[ignore] // Requires database
    async fn test_historical_rate_query() {
        let pool = setup_test_db().await;
        let repo = ExchangeRateRepository::new(pool.clone());
        let provider = Arc::new(FixedRateProvider::new());

        let service = ExchangeRateService::new(repo, ExchangeRateServiceConfig::default())
            .add_provider(provider);

        // Store a rate
        let rate = BigDecimal::from(1);
        service
            .update_rate("NGN", "cNGN", rate.clone(), "test")
            .await
            .unwrap();

        // Query historical rate
        let timestamp = Utc::now();
        let historical_rate = service
            .get_historical_rate("NGN", "cNGN", timestamp)
            .await
            .unwrap();

        assert_eq!(historical_rate, rate);
    }
}
