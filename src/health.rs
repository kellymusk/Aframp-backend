//! Health check module
//! Provides health status for the application and its dependencies

use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tracing::{error, info};

use crate::cache::RedisCache;
use crate::chains::stellar::client::StellarClient;

/// Health status response
#[derive(Debug, Serialize, Clone)]
pub struct HealthStatus {
    pub status: HealthState,
    pub checks: HashMap<String, ComponentHealth>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Overall health state
#[derive(Debug, Serialize, Clone)]
pub enum HealthState {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Individual component health status
#[derive(Debug, Serialize, Clone)]
pub struct ComponentHealth {
    pub status: ComponentState,
    pub response_time_ms: Option<u128>,
    pub details: Option<String>,
}

/// Component state
#[derive(Debug, Serialize, Clone)]
pub enum ComponentState {
    Up,
    Down,
    Warning,
}

impl HealthStatus {
    pub fn new() -> Self {
        Self {
            status: HealthState::Healthy,
            checks: HashMap::new(),
            timestamp: chrono::Utc::now(),
        }
    }

    pub fn is_healthy(&self) -> bool {
        matches!(self.status, HealthState::Healthy)
    }
}

impl ComponentHealth {
    pub fn up(response_time_ms: Option<u128>) -> Self {
        Self {
            status: ComponentState::Up,
            response_time_ms,
            details: None,
        }
    }

    pub fn down(details: Option<String>) -> Self {
        Self {
            status: ComponentState::Down,
            response_time_ms: None,
            details,
        }
    }

    pub fn warning(response_time_ms: Option<u128>, details: Option<String>) -> Self {
        Self {
            status: ComponentState::Warning,
            response_time_ms,
            details,
        }
    }
}

/// Health checker for the application
#[derive(Clone)]
pub struct HealthChecker {
    db_pool: Option<sqlx::PgPool>,
    cache: Option<RedisCache>,
    stellar_client: Option<StellarClient>,
}

impl HealthChecker {
    pub fn new(
        db_pool: Option<sqlx::PgPool>,
        cache: Option<RedisCache>,
        stellar_client: Option<StellarClient>,
    ) -> Self {
        Self {
            db_pool,
            cache,
            stellar_client,
        }
    }

    /// Perform comprehensive health check
    pub async fn check_health(&self) -> HealthStatus {
        let mut health_status = HealthStatus::new();
        let mut overall_healthy = true;
        let mut any_disabled = false;

        // Check database health
        if let Some(db_pool) = &self.db_pool {
            match timeout(Duration::from_secs(5), check_database_health(db_pool)).await {
                Ok(db_result) => match db_result {
                    Ok(response_time) => {
                        health_status.checks.insert(
                            "database".to_string(),
                            ComponentHealth::up(Some(response_time)),
                        );
                        info!("Database health check: OK ({}ms)", response_time);
                    }
                    Err(e) => {
                        overall_healthy = false;
                        health_status.checks.insert(
                            "database".to_string(),
                            ComponentHealth::down(Some(e.to_string())),
                        );
                        error!("Database health check failed: {}", e);
                    }
                },
                Err(_) => {
                    overall_healthy = false;
                    health_status.checks.insert(
                        "database".to_string(),
                        ComponentHealth::down(Some("Timeout".to_string())),
                    );
                    error!("Database health check timed out");
                }
            }
        } else {
            any_disabled = true;
            health_status.checks.insert(
                "database".to_string(),
                ComponentHealth::warning(None, Some("Disabled by configuration".to_string())),
            );
        }

        // Check cache health
        if let Some(cache) = &self.cache {
            match timeout(Duration::from_secs(5), check_cache_health(cache)).await {
                Ok(cache_result) => match cache_result {
                    Ok(response_time) => {
                        health_status.checks.insert(
                            "cache".to_string(),
                            ComponentHealth::up(Some(response_time)),
                        );
                        info!("Cache health check: OK ({}ms)", response_time);
                    }
                    Err(e) => {
                        overall_healthy = false;
                        health_status.checks.insert(
                            "cache".to_string(),
                            ComponentHealth::down(Some(e.to_string())),
                        );
                        error!("Cache health check failed: {}", e);
                    }
                },
                Err(_) => {
                    overall_healthy = false;
                    health_status.checks.insert(
                        "cache".to_string(),
                        ComponentHealth::down(Some("Timeout".to_string())),
                    );
                    error!("Cache health check timed out");
                }
            }
        } else {
            any_disabled = true;
            health_status.checks.insert(
                "cache".to_string(),
                ComponentHealth::warning(None, Some("Disabled by configuration".to_string())),
            );
        }

        // Check Stellar health
        if let Some(stellar_client) = &self.stellar_client {
            match timeout(
                Duration::from_secs(10),
                check_stellar_health(stellar_client),
            )
            .await
            {
                Ok(stellar_result) => match stellar_result {
                    Ok(response_time) => {
                        health_status.checks.insert(
                            "stellar".to_string(),
                            ComponentHealth::up(Some(response_time)),
                        );
                        info!("Stellar health check: OK ({}ms)", response_time);
                    }
                    Err(e) => {
                        overall_healthy = false;
                        health_status.checks.insert(
                            "stellar".to_string(),
                            ComponentHealth::down(Some(e.to_string())),
                        );
                        error!("Stellar health check failed: {}", e);
                    }
                },
                Err(_) => {
                    overall_healthy = false;
                    health_status.checks.insert(
                        "stellar".to_string(),
                        ComponentHealth::down(Some("Timeout".to_string())),
                    );
                    error!("Stellar health check timed out");
                }
            }
        } else {
            any_disabled = true;
            health_status.checks.insert(
                "stellar".to_string(),
                ComponentHealth::warning(None, Some("Disabled by configuration".to_string())),
            );
        }

        // Set overall status
        health_status.status = if overall_healthy {
            if any_disabled {
                HealthState::Degraded
            } else {
                HealthState::Healthy
            }
        } else {
            HealthState::Unhealthy
        };

        health_status
    }
}

// Add a function to check database health
pub async fn check_database_health(
    pool: &sqlx::PgPool,
) -> Result<u128, Box<dyn std::error::Error + Send + Sync>> {
    let start = Instant::now();

    // Try to perform a simple query to check database connectivity
    match sqlx::query("SELECT 1").fetch_one(pool).await {
        Ok(_) => Ok(start.elapsed().as_millis()),
        Err(e) => Err(Box::new(e)),
    }
}

// Add a function to check cache health
pub async fn check_cache_health(
    cache: &RedisCache,
) -> Result<u128, Box<dyn std::error::Error + Send + Sync>> {
    let start = Instant::now();

    // Try to ping the cache to check connectivity
    match cache.get_connection().await {
        Ok(mut conn) => {
            let result: redis::RedisResult<String> =
                redis::cmd("PING").query_async(&mut *conn).await;
            match result {
                Ok(_) => Ok(start.elapsed().as_millis()),
                Err(e) => Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
            }
        }
        Err(e) => Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
    }
}

// Add a function to check Stellar health
pub async fn check_stellar_health(
    stellar_client: &crate::chains::stellar::client::StellarClient,
) -> Result<u128, Box<dyn std::error::Error + Send + Sync>> {
    // Try to perform a simple operation to check Stellar connectivity
    match stellar_client.health_check().await {
        Ok(status) => {
            if status.is_healthy {
                Ok(status.response_time_ms as u128)
            } else {
                Err("Stellar service unhealthy".into())
            }
        }
        Err(e) => Err(Box::new(e)),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_status_creation() {
        let health_status = HealthStatus::new();
        assert!(matches!(health_status.status, HealthState::Healthy));
        assert!(health_status.checks.is_empty());
        assert!(health_status.timestamp <= chrono::Utc::now());
    }

    #[test]
    fn test_component_health_states() {
        let up_health = ComponentHealth::up(Some(100));
        assert!(matches!(up_health.status, ComponentState::Up));
        assert_eq!(up_health.response_time_ms, Some(100));

        let down_health = ComponentHealth::down(Some("Test error".to_string()));
        assert!(matches!(down_health.status, ComponentState::Down));
        assert_eq!(down_health.details, Some("Test error".to_string()));

        let warning_health = ComponentHealth::warning(Some(500), Some("Slow response".to_string()));
        assert!(matches!(warning_health.status, ComponentState::Warning));
        assert_eq!(warning_health.response_time_ms, Some(500));
        assert_eq!(warning_health.details, Some("Slow response".to_string()));
    }
}
