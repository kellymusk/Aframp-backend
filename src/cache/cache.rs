//! Generic cache trait and Redis implementation
//!
//! Provides a type-safe, generic interface for caching operations with:
//! - Automatic JSON serialization/deserialization
//! - Configurable TTL management
//! - Batch operations
//! - Fault tolerance (graceful degradation)
//!

use super::{error::CacheResult, RedisPool};
use crate::cache::CacheError;
use async_trait::async_trait;
use bb8::PooledConnection;
use bb8_redis::RedisConnectionManager;
use redis::AsyncCommands;
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;
use tracing::{debug, warn};

type RedisConnection<'a> = PooledConnection<'a, RedisConnectionManager>;

/// Generic cache trait supporting any serializable type
#[async_trait]
pub trait Cache<T: Serialize + DeserializeOwned + Send + Sync + 'static> {
    /// Get a value from cache by key
    async fn get(&self, key: &str) -> CacheResult<Option<T>>;

    /// Set a value in cache with optional TTL
    async fn set(&self, key: &str, value: &T, ttl: Option<Duration>) -> CacheResult<()>;

    /// Delete a value from cache
    async fn delete(&self, key: &str) -> CacheResult<bool>;

    /// Check if a key exists in cache
    async fn exists(&self, key: &str) -> CacheResult<bool>;

    /// Set multiple key-value pairs with optional TTL
    async fn set_multiple(&self, items: Vec<(String, T)>, ttl: Option<Duration>)
        -> CacheResult<()>;

    /// Get multiple values by keys
    async fn get_multiple(&self, keys: Vec<String>) -> CacheResult<Vec<Option<T>>>;

    /// Increment a numeric value (for counters)
    async fn increment(&self, key: &str, amount: i64) -> CacheResult<i64>;

    /// Decrement a numeric value (for counters)
    async fn decrement(&self, key: &str, amount: i64) -> CacheResult<i64>;

    /// Set expiration time for a key
    async fn expire(&self, key: &str, ttl: Duration) -> CacheResult<bool>;

    /// Get time-to-live for a key in seconds
    async fn ttl(&self, key: &str) -> CacheResult<i64>;

    /// Delete multiple keys by pattern
    async fn delete_pattern(&self, pattern: &str) -> CacheResult<u64>;
}

/// Redis implementation of the Cache trait
pub struct RedisCache {
    pool: RedisPool,
}

impl RedisCache {
    /// Create a new Redis cache instance
    pub fn new(pool: RedisPool) -> Self {
        Self { pool }
    }

    /// Get a connection from the pool with error handling
    async fn get_connection(&self) -> CacheResult<RedisConnection<'_>> {
        self.pool.get().await.map_err(|e| {
            warn!("Failed to get Redis connection: {}", e);
            e.into()
        })
    }
}

#[async_trait]
impl<T: Serialize + DeserializeOwned + Send + Sync + 'static> Cache<T> for RedisCache {
    async fn get(&self, key: &str) -> CacheResult<Option<T>> {
        let mut conn = match self.get_connection().await {
            Ok(conn) => conn,
            Err(_) => return Ok(None), // Graceful degradation
        };

        let result: Option<String> = conn.get(key).await.map_err(|e| {
            warn!("Redis GET failed for key '{}': {}", key, e);
            e
        })?;

        match result {
            Some(json_str) => {
                let value: T = serde_json::from_str(&json_str)
                    .map_err(|e| {
                        warn!("Failed to deserialize cache value for key '{}': {}", key, e);
                        <serde_json::Error as Into<CacheError>>::into(e)
                    })
                    .unwrap();
                debug!("Cache hit for key: {}", key);
                Ok(Some(value))
            }
            None => {
                debug!("Cache miss for key: {}", key);
                Ok(None)
            }
        }
    }

    async fn set(&self, key: &str, value: &T, ttl: Option<Duration>) -> CacheResult<()> {
        let mut conn = match self.get_connection().await {
            Ok(conn) => conn,
            Err(_) => return Ok(()), // Graceful degradation - don't fail
        };

        let json_str = serde_json::to_string(value).map_err(|e| {
            warn!("Failed to serialize value for key '{}': {}", key, e);
            e
        })?;

        match ttl {
            Some(ttl_duration) => {
                let ttl_seconds = ttl_duration.as_secs() as usize;
                let _: () = conn
                    .set_ex(key, json_str, ttl_seconds as u64)
                    .await
                    .map_err(|e| {
                        warn!("Redis SET_EX failed for key '{}': {}", key, e);
                        e
                    })?;
            }
            None => {
                let _: () = conn.set(key, json_str).await.map_err(|e| {
                    warn!("Redis SET failed for key '{}': {}", key, e);
                    e
                })?;
            }
        }

        debug!("Cache set for key: {} (ttl: {:?})", key, ttl);
        Ok(())
    }

    async fn delete(&self, key: &str) -> CacheResult<bool> {
        let mut conn = match self.get_connection().await {
            Ok(conn) => conn,
            Err(_) => return Ok(false), // Graceful degradation
        };

        let result: i32 = conn.del(key).await.map_err(|e| {
            warn!("Redis DEL failed for key '{}': {}", key, e);
            e
        })?;

        let deleted = result > 0;
        if deleted {
            debug!("Cache delete for key: {}", key);
        }
        Ok(deleted)
    }

    async fn exists(&self, key: &str) -> CacheResult<bool> {
        let mut conn = match self.get_connection().await {
            Ok(conn) => conn,
            Err(_) => return Ok(false), // Graceful degradation
        };

        let result: i32 = conn.exists(key).await.map_err(|e| {
            warn!("Redis EXISTS failed for key '{}': {}", key, e);
            e
        })?;

        Ok(result > 0)
    }

    async fn set_multiple(
        &self,
        items: Vec<(String, T)>,
        ttl: Option<Duration>,
    ) -> CacheResult<()> {
        if items.is_empty() {
            return Ok(());
        }

        let item_count = items.len();

        let mut conn = match self.get_connection().await {
            Ok(conn) => conn,
            Err(_) => return Ok(()), // Graceful degradation
        };

        let mut pipeline = redis::pipe();

        for (key, value) in items {
            let json_str = serde_json::to_string(&value).map_err(|e| {
                warn!("Failed to serialize value for key '{}': {}", key, e);
                e
            })?;

            if let Some(ttl_duration) = ttl {
                let ttl_seconds = ttl_duration.as_secs();
                pipeline.set_ex(&key, json_str, ttl_seconds);
            } else {
                pipeline.set(&key, json_str);
            }
        }

        let _: () = pipeline.query_async(&mut *conn).await.map_err(|e| {
            warn!("Redis MSET failed: {}", e);
            e
        })?;

        debug!("Cache set_multiple with {} items", item_count);
        Ok(())
    }

    async fn get_multiple(&self, keys: Vec<String>) -> CacheResult<Vec<Option<T>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let mut conn = match self.get_connection().await {
            Ok(conn) => conn,
            Err(_) => {
                // Graceful degradation - return empty Option<T> for all keys
                let mut result = Vec::with_capacity(keys.len());
                for _ in 0..keys.len() {
                    result.push(None);
                }
                return Ok(result);
            }
        };

        let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
        let results: Vec<Option<String>> = conn.get(key_refs).await.map_err(|e| {
            warn!("Redis MGET failed: {}", e);
            e
        })?;

        let mut deserialized = Vec::with_capacity(results.len());
        for (i, result) in results.into_iter().enumerate() {
            match result {
                Some(json_str) => match serde_json::from_str(&json_str) {
                    Ok(value) => deserialized.push(Some(value)),
                    Err(e) => {
                        warn!(
                            "Failed to deserialize cache value for key '{}': {}",
                            keys[i], e
                        );
                        deserialized.push(None);
                    }
                },
                None => deserialized.push(None),
            }
        }

        debug!("Cache get_multiple for {} keys", keys.len());
        Ok(deserialized)
    }

    async fn increment(&self, key: &str, amount: i64) -> CacheResult<i64> {
        let mut conn = match self.get_connection().await {
            Ok(conn) => conn,
            Err(_) => return Ok(0), // Graceful degradation
        };

        let result: i64 = if amount == 1 {
            conn.incr(key, 1).await
        } else {
            conn.incr(key, amount).await
        }
        .map_err(|e| {
            warn!("Redis INCR failed for key '{}': {}", key, e);
            e
        })?;

        debug!("Cache increment for key: {} by {}", key, amount);
        Ok(result)
    }

    async fn decrement(&self, key: &str, amount: i64) -> CacheResult<i64> {
        let mut conn = match self.get_connection().await {
            Ok(conn) => conn,
            Err(_) => return Ok(0), // Graceful degradation
        };

        let result: i64 = if amount == 1 {
            conn.decr(key, 1).await
        } else {
            conn.decr(key, amount).await
        }
        .map_err(|e| {
            warn!("Redis DECR failed for key '{}': {}", key, e);
            e
        })?;

        debug!("Cache decrement for key: {} by {}", key, amount);
        Ok(result)
    }

    async fn expire(&self, key: &str, ttl: Duration) -> CacheResult<bool> {
        let mut conn = match self.get_connection().await {
            Ok(conn) => conn,
            Err(_) => return Ok(false), // Graceful degradation
        };

        let ttl_seconds = ttl.as_secs();
        // Redis expire expects i64 (seconds)
        if ttl_seconds > i64::MAX as u64 {
            return Err(CacheError::TtlError("TTL too large".to_string()));
        }
        let result: i32 = conn.expire(key, ttl_seconds as i64).await.map_err(|e| {
            warn!("Redis EXPIRE failed for key '{}': {}", key, e);
            e
        })?;

        let success = result > 0;
        if success {
            debug!("Cache expire set for key: {} to {}s", key, ttl_seconds);
        }
        Ok(success)
    }

    async fn ttl(&self, key: &str) -> CacheResult<i64> {
        let mut conn = match self.get_connection().await {
            Ok(conn) => conn,
            Err(_) => return Ok(-2), // Return -2 to indicate key doesn't exist (graceful degradation)
        };

        let result: i64 = conn.ttl(key).await.map_err(|e| {
            warn!("Redis TTL failed for key '{}': {}", key, e);
            e
        })?;

        Ok(result)
    }

    async fn delete_pattern(&self, pattern: &str) -> CacheResult<u64> {
        let mut conn = match self.get_connection().await {
            Ok(conn) => conn,
            Err(_) => return Ok(0), // Graceful degradation
        };

        // Get all keys matching the pattern
        let keys: Vec<String> = conn.keys(pattern).await.map_err(|e| {
            warn!("Redis KEYS failed for pattern '{}': {}", pattern, e);
            e
        })?;

        if keys.is_empty() {
            return Ok(0);
        }

        // Delete all matching keys
        let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
        let result: i32 = conn.del(key_refs).await.map_err(|e| {
            warn!("Redis DEL failed for pattern '{}': {}", pattern, e);
            e
        })?;

        let deleted = result as u64;
        debug!(
            "Cache delete_pattern '{}' deleted {} keys",
            pattern, deleted
        );
        Ok(deleted)
    }
}

/// TTL constants for different data types
pub mod ttl {
    use std::time::Duration;

    /// Exchange rates: 1-2 minutes
    pub const EXCHANGE_RATES: Duration = Duration::from_secs(90); // 1.5 minutes

    /// Wallet balances: 30-60 seconds
    pub const WALLET_BALANCES: Duration = Duration::from_secs(45);

    /// AFRI trustlines: 1 hour
    pub const TRUSTLINES: Duration = Duration::from_secs(3600);

    /// Fee structures: 1 hour
    pub const FEE_STRUCTURES: Duration = Duration::from_secs(3600);

    /// User sessions: 30 minutes
    pub const USER_SESSIONS: Duration = Duration::from_secs(1800);

    /// Transaction status: 5 minutes
    pub const TRANSACTION_STATUS: Duration = Duration::from_secs(300);

    /// Recent transactions: 10 minutes
    pub const RECENT_TRANSACTIONS: Duration = Duration::from_secs(600);

    /// JWT validation: 15 minutes
    pub const JWT_VALIDATION: Duration = Duration::from_secs(900);

    /// Rate limiting: 1 minute
    pub const RATE_LIMITING: Duration = Duration::from_secs(60);

    /// Bill payment providers: 30 minutes
    pub const BILL_PROVIDERS: Duration = Duration::from_secs(1800);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestData {
        id: u32,
        name: String,
    }

    // Note: These tests require a running Redis instance
    // Run with: REDIS_URL=redis://localhost:6379 cargo test --features cache

    #[tokio::test]
    #[ignore] // Requires Redis
    async fn test_basic_cache_operations() {
        let pool = super::super::init_cache_pool(super::super::CacheConfig::default())
            .await
            .unwrap();
        let cache: RedisCache = RedisCache::new(pool);

        let test_data = TestData {
            id: 1,
            name: "test".to_string(),
        };

        // Test set and get
        cache
            .set("test:key", &test_data, Some(Duration::from_secs(60)))
            .await
            .unwrap();
        let retrieved = cache.get("test:key").await.unwrap();
        assert_eq!(retrieved, Some(test_data));

        // Test exists
        assert!(<RedisCache as Cache<TestData>>::exists(&cache, "test:key")
            .await
            .unwrap());

        // Test delete
        assert!(<RedisCache as Cache<TestData>>::delete(&cache, "test:key")
            .await
            .unwrap());
        assert!(!<RedisCache as Cache<TestData>>::exists(&cache, "test:key")
            .await
            .unwrap());
    }

    #[tokio::test]
    #[ignore] // Requires Redis
    async fn test_increment_decrement() {
        let pool = super::super::init_cache_pool(super::super::CacheConfig::default())
            .await
            .unwrap();
        let cache = RedisCache::new(pool);

        let key = "test:counter";

        // Clean up
        let _ = <RedisCache as Cache<String>>::delete(&cache, key).await;

        // Test increment
        let result = <RedisCache as Cache<String>>::increment(&cache, key, 1)
            .await
            .unwrap();
        assert_eq!(result, 1);

        let result = <RedisCache as Cache<String>>::increment(&cache, key, 5)
            .await
            .unwrap();
        assert_eq!(result, 6);

        // Test decrement
        let result = <RedisCache as Cache<String>>::decrement(&cache, key, 2)
            .await
            .unwrap();
        assert_eq!(result, 4);

        // Clean up
        let _ = <RedisCache as Cache<String>>::delete(&cache, key).await;
    }
}
