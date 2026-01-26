//! Integration tests for Redis caching layer
//!
//! These tests require a running Redis instance and database.
//! Run with: REDIS_URL=redis://localhost:6379 DATABASE_URL=postgres://... cargo test --features cache --test cache_integration_test

#[cfg(feature = "cache")]
mod cache_tests {
    use std::time::Duration;
    use Bitmesh_backend::cache::{cache::Cache, keys::*, CacheConfig, RedisCache};
    use Bitmesh_backend::database::{
        exchange_rate_repository::ExchangeRateRepository, init_pool,
        trustline_repository::TrustlineRepository, wallet_repository::WalletRepository, PoolConfig,
    };

    async fn setup_cache() -> RedisCache {
        let config = CacheConfig {
            redis_url: std::env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string()),
            ..Default::default()
        };

        let pool = Bitmesh_backend::cache::init_cache_pool(config)
            .await
            .expect("Failed to init cache pool");
        RedisCache::new(pool)
    }

    async fn setup_db() -> sqlx::PgPool {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let config = PoolConfig::default();
        init_pool(&database_url, Some(config))
            .await
            .expect("Failed to init DB pool")
    }

    #[tokio::test]
    async fn test_exchange_rate_caching() {
        let cache = setup_cache().await;
        let db_pool = setup_db().await;

        let mut repo = ExchangeRateRepository::new(db_pool);
        repo.enable_cache(cache);

        // Test caching a rate
        let _rate = repo
            .upsert_rate("AFRI", "USD", "0.85", Some("external_api"))
            .await
            .unwrap();

        // First call should cache the result
        let cached_rate = repo.get_current_rate("AFRI", "USD").await.unwrap();
        assert_eq!(cached_rate.as_ref().unwrap().rate, "0.85");

        // Second call should hit cache
        let cached_rate2 = repo.get_current_rate("AFRI", "USD").await.unwrap();
        assert_eq!(cached_rate2.as_ref().unwrap().rate, "0.85");

        // Update rate should invalidate cache
        repo.upsert_rate("AFRI", "USD", "0.90", Some("external_api"))
            .await
            .unwrap();

        let updated_rate = repo.get_current_rate("AFRI", "USD").await.unwrap();
        assert_eq!(updated_rate.as_ref().unwrap().rate, "0.90");
    }

    #[tokio::test]
    async fn test_wallet_balance_caching() {
        let cache = setup_cache().await;
        let db_pool = setup_db().await;

        let mut repo = WalletRepository::new(db_pool);
        repo.enable_cache(cache);

        // Create a test wallet
        let wallet = repo
            .create_wallet("test_user", "GA123456789", "100.00")
            .await
            .unwrap();

        // First balance check should cache
        let wallet_data = repo.find_by_account("GA123456789").await.unwrap().unwrap();
        assert_eq!(wallet_data.balance, "100.00");

        // Update balance should invalidate cache
        repo.update_balance(&wallet.id, "150.00").await.unwrap();

        let updated_wallet = repo.find_by_account("GA123456789").await.unwrap().unwrap();
        assert_eq!(updated_wallet.balance, "150.00");

        // Cleanup
        repo.delete(&wallet.id).await.unwrap();
    }

    #[tokio::test]
    async fn test_trustline_caching() {
        let cache = setup_cache().await;
        let db_pool = setup_db().await;

        let mut repo = TrustlineRepository::new(db_pool);
        repo.enable_cache(cache);

        // Create a test trustline
        let trustline = repo
            .create_trustline("GA123456789", "AFRI", "issuer_address", "1000.00")
            .await
            .unwrap();

        // First check should cache existence
        let found_trustline = repo
            .find_trustline("GA123456789", "AFRI")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found_trustline.status, "pending");

        // Update status
        repo.update_status(&trustline.id, "active").await.unwrap();

        let updated_trustline = repo
            .find_trustline("GA123456789", "AFRI")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated_trustline.status, "active");

        // Cleanup
        repo.delete(&trustline.id).await.unwrap();
    }

    #[tokio::test]
    async fn test_cache_ttl_expiration() {
        let cache = setup_cache().await;

        // Test basic cache operations with TTL
        let test_data = "test_value".to_string();
        let ttl = Duration::from_secs(2);

        cache
            .set("test:ttl:key", &test_data, Some(ttl))
            .await
            .unwrap();

        // Should exist immediately
        let retrieved = cache.get("test:ttl:key").await.unwrap();
        assert_eq!(retrieved, Some(test_data));

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Should be gone
        let expired = <RedisCache as Cache<String>>::get(&cache, "test:ttl:key")
            .await
            .unwrap();
        assert_eq!(expired, None);
    }

    #[tokio::test]
    async fn test_cache_batch_operations() {
        let cache = setup_cache().await;

        // Test batch set
        let items = vec![
            ("batch:key1".to_string(), "value1".to_string()),
            ("batch:key2".to_string(), "value2".to_string()),
            ("batch:key3".to_string(), "value3".to_string()),
        ];

        cache
            .set_multiple(items, Some(Duration::from_secs(60)))
            .await
            .unwrap();

        // Test batch get
        let keys = vec![
            "batch:key1".to_string(),
            "batch:key2".to_string(),
            "batch:key3".to_string(),
        ];

        let results = cache.get_multiple(keys).await.unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0], Some("value1".to_string()));
        assert_eq!(results[1], Some("value2".to_string()));
        assert_eq!(results[2], Some("value3".to_string()));
    }

    #[tokio::test]
    async fn test_cache_key_generation() {
        // Test that our key builders generate expected strings
        let balance_key = wallet::BalanceKey::new("GA123456789");
        assert_eq!(balance_key.to_string(), "v1:wallet:balance:GA123456789");

        let rate_key = exchange_rate::CurrencyPairKey::afri_rate("USD");
        assert_eq!(rate_key.to_string(), "v1:rate:AFRI:USD");

        let trustline_key = wallet::TrustlineKey::new("GA123456789");
        assert_eq!(trustline_key.to_string(), "v1:wallet:trustline:GA123456789");
    }

    #[tokio::test]
    async fn test_graceful_degradation() {
        // Test that operations work even when Redis is unavailable
        // This simulates Redis being down

        // Create a cache with invalid Redis URL
        let config = CacheConfig {
            redis_url: "redis://invalid-host:6379".to_string(),
            connection_timeout: Duration::from_millis(100), // Fast timeout
            ..Default::default()
        };

        // This should fail to connect but not panic
        let result = Bitmesh_backend::cache::init_cache_pool(config).await;
        assert!(
            result.is_err(),
            "Expected cache initialization to fail with invalid Redis URL"
        );

        // In a real application, repositories would continue to work with database-only
        // This test verifies that cache initialization failures are handled gracefully
    }
}
