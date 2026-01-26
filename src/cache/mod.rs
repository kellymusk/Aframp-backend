//! Redis-based caching layer for Aframp

pub mod cache;
pub mod error;
pub mod keys;

// Re-export commonly used items
pub use cache::{Cache, RedisCache};
pub use error::CacheError;

use bb8::Pool;
use bb8_redis::RedisConnectionManager;
use std::time::Duration;
use tracing::{error, info, warn};

pub type RedisPool = Pool<RedisConnectionManager>;

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub redis_url: String,
    pub max_connections: u32,
    pub min_idle: u32,
    pub connection_timeout: Duration,
    pub max_lifetime: Duration,
    pub idle_timeout: Duration,
    pub health_check_interval: Duration,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            redis_url: "redis://127.0.0.1:6379".to_string(),
            max_connections: 20,
            min_idle: 5,
            connection_timeout: Duration::from_secs(5),
            max_lifetime: Duration::from_secs(300),
            idle_timeout: Duration::from_secs(60),
            health_check_interval: Duration::from_secs(30),
        }
    }
}

pub async fn init_cache_pool(config: CacheConfig) -> Result<RedisPool, CacheError> {
    info!(
        "Initializing Redis cache pool: max_connections={}, redis_url={}",
        config.max_connections, config.redis_url
    );

    let manager = RedisConnectionManager::new(config.redis_url.clone()).map_err(|e| {
        error!("Failed to create Redis connection manager: {}", e);
        CacheError::ConnectionError(e.to_string())
    })?;

    let pool = Pool::builder()
        .max_size(config.max_connections)
        .min_idle(config.min_idle)
        .connection_timeout(config.connection_timeout)
        .max_lifetime(config.max_lifetime)
        .idle_timeout(config.idle_timeout)
        .test_on_check_out(false)
        .build(manager)
        .await
        .map_err(|e| {
            error!("Failed to build Redis connection pool: {}", e);
            CacheError::ConnectionError(e.to_string())
        })?;

    if let Err(e) = test_connection(&pool).await {
        warn!(
            "Initial Redis connection test failed, but continuing: {}",
            e
        );
    }

    info!("Redis cache pool initialized successfully");
    Ok(pool)
}

///
async fn test_connection(pool: &RedisPool) -> Result<(), CacheError> {
    let mut conn = pool.get().await.map_err(|e| {
        error!("Failed to get Redis connection for test: {}", e);
        CacheError::ConnectionError(e.to_string())
    })?;

    let _: String = redis::cmd("PING")
        .query_async(&mut *conn)
        .await
        .map_err(|e| {
            error!("Redis PING failed: {}", e);
            CacheError::ConnectionError(e.to_string())
        })?;

    Ok(())
}

pub async fn health_check(pool: &RedisPool) -> Result<(), CacheError> {
    test_connection(pool).await
}

#[derive(Debug)]
pub struct CacheStats {
    pub connections: u32,
    pub idle_connections: u32,
    pub connections_in_use: u32,
}

pub fn get_cache_stats(pool: &RedisPool) -> CacheStats {
    CacheStats {
        connections: pool.state().connections as u32,
        idle_connections: pool.state().idle_connections as u32,
        connections_in_use: (pool.state().connections - pool.state().idle_connections) as u32,
    }
}

pub async fn shutdown_cache_pool(_pool: &RedisPool) {
    info!("Shutting down Redis cache pool");
    // bb8 pools are dropped automatically when they go out of scope
}
