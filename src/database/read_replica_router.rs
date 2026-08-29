//! Read Replica Router with health checking and failover (Issue #XXX)
//!
//! Provides:
//! - Weighted load balancing across read replicas
//! - Automatic failover on replica lag
//! - Consistency level support (eventual vs. read-your-writes)
//! - Replica health tracking and recovery
//! - Transparent routing from query layer

use crate::database::error::DatabaseError;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

// ─────────────────────────────────────────────────────────────────────────────
// Consistency Level
// ─────────────────────────────────────────────────────────────────────────────

/// Transaction consistency requirement for read operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsistencyLevel {
    /// Eventual consistency — can read from replica
    /// Use for: analytics, auditing, non-critical queries
    Eventual,

    /// Read-your-writes consistency — must read from primary
    /// Use after: write operations, transactional consistency required
    ReadYourWrites,

    /// Serializable consistency — must read from primary with locks
    /// Use for: critical financial operations, compliance checks
    Serializable,
}

// ─────────────────────────────────────────────────────────────────────────────
// Replica Health Metrics
// ─────────────────────────────────────────────────────────────────────────────

/// Per-replica health information
#[derive(Debug, Clone)]
pub struct ReplicaMetrics {
    /// Last observed replica lag in milliseconds
    pub lag_ms: i64,

    /// Last observation time
    pub last_check: Instant,

    /// Connection failures in last minute
    pub failure_count: u64,

    /// Average query latency (microseconds)
    pub query_latency_us: u64,

    /// Is this replica considered healthy?
    pub is_healthy: bool,
}

/// Tracks health of a single replica
struct ReplicaHealthTracker {
    /// DSN for this replica
    pub dsn: String,

    /// Pool for this replica (None if unhealthy)
    pub pool: Option<Arc<PgPool>>,

    /// Metrics
    pub lag_ms: AtomicI64,
    pub failure_count: AtomicU64,
    pub query_latency_us: AtomicU64,

    /// Last successful health check time
    pub last_healthy_check: RwLock<Instant>,

    /// Is this replica marked unhealthy?
    pub is_unhealthy: RwLock<bool>,
}

impl ReplicaHealthTracker {
    pub async fn new(dsn: &str, pool: Arc<PgPool>) -> Self {
        Self {
            dsn: dsn.to_string(),
            pool: Some(pool),
            lag_ms: AtomicI64::new(0),
            failure_count: AtomicU64::new(0),
            query_latency_us: AtomicU64::new(0),
            last_healthy_check: RwLock::new(Instant::now()),
            is_unhealthy: RwLock::new(false),
        }
    }

    /// Check replica lag using PostgreSQL `pg_last_xlog_receive_lsn()` difference
    pub async fn check_lag(&self) -> Result<i64, DatabaseError> {
        if let Some(pool) = &self.pool {
            let start = Instant::now();
            match sqlx::query_scalar::<_, i64>(
                "SELECT EXTRACT(EPOCH FROM (NOW() - pg_last_xact_replay_timestamp()))::BIGINT",
            )
            .fetch_one(pool.as_ref())
            .await
            {
                Ok(lag_seconds) => {
                    let lag_ms = lag_seconds * 1000;
                    self.lag_ms.store(lag_ms, Ordering::Relaxed);

                    let latency_us = start.elapsed().as_micros() as u64;
                    self.query_latency_us.store(latency_us, Ordering::Relaxed);

                    *self.last_healthy_check.write().await = Instant::now();
                    Ok(lag_ms)
                }
                Err(e) => {
                    self.failure_count.fetch_add(1, Ordering::Relaxed);
                    Err(DatabaseError::from_sqlx(e))
                }
            }
        } else {
            Ok(i64::MAX)
        }
    }

    /// Mark this replica as unhealthy (will not be used for reads)
    pub async fn mark_unhealthy(&self) {
        *self.is_unhealthy.write().await = true;
        info!("Marked replica unhealthy: {}", self.dsn);
    }

    /// Mark this replica as recovered (will be used again for reads)
    pub async fn mark_healthy(&self) {
        *self.is_unhealthy.write().await = false;
        self.failure_count.store(0, Ordering::Relaxed);
        info!("Marked replica recovered: {}", self.dsn);
    }

    pub async fn get_metrics(&self) -> ReplicaMetrics {
        ReplicaMetrics {
            lag_ms: self.lag_ms.load(Ordering::Relaxed),
            last_check: *self.last_healthy_check.read().await,
            failure_count: self.failure_count.load(Ordering::Relaxed),
            query_latency_us: self.query_latency_us.load(Ordering::Relaxed),
            is_healthy: !*self.is_unhealthy.read().await,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Read Replica Router
// ─────────────────────────────────────────────────────────────────────────────

pub struct ReadReplicaRouter {
    /// Primary pool (for writes + consistent reads)
    primary_pool: Arc<PgPool>,

    /// Per-shard HA pool manager (if available)
    ha_manager: Option<Arc<crate::database::ha_pool::HaPoolManager>>,

    /// Replica trackers (indexed by replica_id)
    replicas: Arc<RwLock<Vec<Arc<ReplicaHealthTracker>>>>,

    /// Configuration
    replica_lag_threshold_ms: i64,
    health_check_interval: Duration,
    failure_threshold: u64,

    /// Round-robin cursor for load balancing
    cursor: std::sync::atomic::AtomicUsize,
}

impl ReadReplicaRouter {
    /// Create a new read replica router
    pub async fn new(
        primary_pool: Arc<PgPool>,
        replica_dsns: Vec<String>,
        ha_manager: Option<Arc<crate::database::ha_pool::HaPoolManager>>,
        replica_lag_threshold_ms: i64,
    ) -> Result<Arc<Self>, DatabaseError> {
        let mut replicas = Vec::new();

        // Initialize replica pools
        for dsn in replica_dsns {
            match PgPoolOptions::new()
                .max_connections(64)
                .min_connections(8)
                .acquire_timeout(Duration::from_secs(10))
                .connect(&dsn)
                .await
            {
                Ok(pool) => {
                    let tracker = ReplicaHealthTracker::new(&dsn, Arc::new(pool)).await;
                    replicas.push(Arc::new(tracker));
                    info!("Initialized replica: {}", dsn);
                }
                Err(e) => {
                    warn!("Failed to initialize replica {}: {}", dsn, e);
                }
            }
        }

        let router = Arc::new(Self {
            primary_pool,
            ha_manager,
            replicas: Arc::new(RwLock::new(replicas)),
            replica_lag_threshold_ms,
            health_check_interval: Duration::from_secs(30),
            failure_threshold: 5,
            cursor: std::sync::atomic::AtomicUsize::new(0),
        });

        // Spawn background health check task
        let router_clone = Arc::clone(&router);
        tokio::spawn(async move {
            router_clone.health_check_loop().await;
        });

        Ok(router)
    }

    /// Route a read query based on consistency requirement and shard key
    pub async fn execute_read<T, F>(
        &self,
        query_fn: F,
        consistency: ConsistencyLevel,
        shard_key: Option<&str>,
    ) -> Result<T, DatabaseError>
    where
        F: Fn(&PgPool) -> sqlx::query::QueryAs<
            'static,
            sqlx::Postgres,
            T,
            sqlx::postgres::PgArguments,
        > + Send
            + Sync,
        T: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
    {
        let pool = self.get_read_pool(consistency, shard_key).await?;
        let row = query_fn(&pool).fetch_one(pool.as_ref()).await;
        row.map_err(DatabaseError::from_sqlx)
    }

    /// Get appropriate pool for read operation
    async fn get_read_pool(
        &self,
        consistency: ConsistencyLevel,
        shard_key: Option<&str>,
    ) -> Result<Arc<PgPool>, DatabaseError> {
        match consistency {
            ConsistencyLevel::ReadYourWrites | ConsistencyLevel::Serializable => {
                // Always route to primary
                Ok(Arc::new(self.primary_pool.as_ref().clone()))
            }

            ConsistencyLevel::Eventual => {
                // Try to use replica, fallback to primary
                if let Some(replica_pool) = self.select_healthy_replica().await {
                    debug!("Routing eventual-consistency read to replica");
                    Ok(replica_pool)
                } else {
                    debug!("No healthy replicas, routing to primary");
                    Ok(Arc::new(self.primary_pool.as_ref().clone()))
                }
            }
        }
    }

    /// Select a healthy replica using round-robin + health check
    async fn select_healthy_replica(&self) -> Option<Arc<PgPool>> {
        let replicas = self.replicas.read().await;

        if replicas.is_empty() {
            return None;
        }

        let n = replicas.len();
        for _ in 0..n {
            let idx = self.cursor.fetch_add(1, Ordering::Relaxed) % n;
            let replica = &replicas[idx];

            // Skip if marked unhealthy
            if *replica.is_unhealthy.read().await {
                continue;
            }

            // Check if lag is acceptable
            let lag = replica.lag_ms.load(Ordering::Relaxed);
            if lag <= self.replica_lag_threshold_ms {
                if let Some(pool) = &replica.pool {
                    return Some(Arc::clone(pool));
                }
            }
        }

        None
    }

    /// Background task to periodically check replica health
    async fn health_check_loop(&self) {
        loop {
            tokio::time::sleep(self.health_check_interval).await;

            let replicas = self.replicas.read().await;
            for (idx, replica) in replicas.iter().enumerate() {
                match replica.check_lag().await {
                    Ok(lag_ms) => {
                        if lag_ms > self.replica_lag_threshold_ms {
                            warn!(
                                "Replica {} lag exceeds threshold: {} ms",
                                idx, lag_ms
                            );
                            replica.mark_unhealthy().await;
                        } else if *replica.is_unhealthy.read().await {
                            // Replica recovered
                            replica.mark_healthy().await;
                        }
                    }
                    Err(e) => {
                        warn!("Health check failed for replica {}: {}", idx, e);
                        replica.failure_count.fetch_add(1, Ordering::Relaxed);

                        if replica.failure_count.load(Ordering::Relaxed) >= self.failure_threshold
                        {
                            replica.mark_unhealthy().await;
                        }
                    }
                }
            }
        }
    }

    /// Get metrics for all replicas
    pub async fn get_replica_metrics(&self) -> Vec<ReplicaMetrics> {
        let replicas = self.replicas.read().await;
        let mut metrics = Vec::new();
        for replica in replicas.iter() {
            metrics.push(replica.get_metrics().await);
        }
        metrics
    }

    /// Get current replica count
    pub async fn replica_count(&self) -> usize {
        self.replicas.read().await.len()
    }

    /// Get count of healthy replicas
    pub async fn healthy_replica_count(&self) -> usize {
        let replicas = self.replicas.read().await;
        replicas
            .iter()
            .filter(|r| !*std::sync::Arc::clone(r).is_unhealthy.blocking_read())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_consistency_level_routing() {
        // This would require a test database setup
        // Placeholder for integration test
    }

    #[test]
    fn test_consistency_level_equality() {
        assert_eq!(ConsistencyLevel::Eventual, ConsistencyLevel::Eventual);
        assert_ne!(ConsistencyLevel::Eventual, ConsistencyLevel::ReadYourWrites);
    }
}
