// This module requires std library (not available in WASM)

pub mod bill_payment_repository;
pub mod conversion_audit_repository;
pub mod error;
pub mod exchange_rate_repository;
pub mod fee_structure_repository;
pub mod payment_method_repository;
pub mod payment_repository;
pub mod provider_config_repository;
pub mod repository;
pub mod transaction;
pub mod transaction_repository;
pub mod trustline_operation_repository;
pub mod trustline_repository;
pub mod wallet_repository;
pub mod webhook_repository;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::time::Duration;
use tracing::{error as log_error, info, warn};

use self::error::DatabaseError;
use crate::config::DatabaseConfig;

/// Database pool configuration
#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub connection_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_lifetime: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 20,
            min_connections: 5,
            connection_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(600),
            max_lifetime: Duration::from_secs(1800),
        }
    }
}

/// Initialize the database connection pool
pub async fn init_pool(
    database_url: &str,
    config: Option<PoolConfig>,
) -> Result<PgPool, DatabaseError> {
    let config = config.unwrap_or_default();

    info!(
        "Initializing database pool: max_connections={}, min_connections={}, connection_timeout={:?}",
        config.max_connections, config.min_connections, config.connection_timeout
    );

    let pool = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(config.connection_timeout)
        .idle_timeout(config.idle_timeout)
        .max_lifetime(config.max_lifetime)
        .connect(database_url)
        .await
        .map_err(|e| {
            log_error!("Failed to initialize database pool: {}", e);
            DatabaseError::from_sqlx(e)
        })?;

    // Test the connection
    pool.acquire().await.map_err(|e| {
        log_error!("Failed to acquire test connection: {}", e);
        DatabaseError::from_sqlx(e)
    })?;

    info!("Database pool initialized successfully");
    Ok(pool)
}

/// Connection pool health check
pub async fn health_check(pool: &PgPool) -> Result<(), DatabaseError> {
    let _result = sqlx::query("SELECT 1").fetch_one(pool).await.map_err(|e| {
        warn!("Health check failed: {}", e);
        DatabaseError::from_sqlx(e)
    })?;

    Ok(())
}

/// Get pool statistics
pub struct PoolStats {
    pub num_idle: u32,
    pub size: u32,
}

pub fn get_pool_stats(pool: &PgPool) -> PoolStats {
    PoolStats {
        num_idle: pool.num_idle() as u32,
        size: pool.size(),
    }
}

/// Initialize the database pool from application configuration
pub async fn init_pool_from_config(config: &DatabaseConfig) -> Result<PgPool, DatabaseError> {
    let pool_config = PoolConfig {
        max_connections: config.max_connections,
        min_connections: config.min_connections,
        connection_timeout: Duration::from_secs(config.connection_timeout),
        idle_timeout: Duration::from_secs(config.idle_timeout.unwrap_or(600)),
        max_lifetime: Duration::from_secs(1800),
    };

    init_pool(&config.url, Some(pool_config)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database running
    async fn test_pool_initialization() {
        let url = "postgres://user:password@localhost:5432/aframp";
        let config = PoolConfig::default();
        let _result = init_pool(url, Some(config)).await;
        // This test requires actual database to be running
        // assert!(result.is_ok());
    }

    #[test]
    fn test_default_pool_config() {
        let config = PoolConfig::default();
        assert_eq!(config.max_connections, 20);
        assert_eq!(config.min_connections, 5);
        assert_eq!(config.connection_timeout, Duration::from_secs(30));
    }
}
