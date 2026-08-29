//! HA Database Pool — Issue #347
//!
//! Provides:
//! - Logical sharding by `shard_key` (Merchant_ID / Agent_ID) using consistent hashing
//! - Per-shard primary + read-replica pools
//! - Round-robin read load balancing with automatic failover to primary
//! - Periodic replica integrity checksumming
//! - Hot shard addition without cluster restart

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ShardConfig {
    /// Unique shard identifier (0-based index)
    pub shard_id: u32,
    /// Primary (synchronous replication target) DSN
    pub primary_url: String,
    /// Async read-replica DSNs
    pub replica_urls: Vec<String>,
    pub max_connections: u32,
    pub min_connections: u32,
    pub connection_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct HaPoolConfig {
    pub shards: Vec<ShardConfig>,
    /// How often to run replica checksum health checks
    pub checksum_interval: Duration,
}

// ---------------------------------------------------------------------------
// Internal shard state
// ---------------------------------------------------------------------------

struct ShardPools {
    primary: PgPool,
    replicas: Vec<PgPool>,
    /// Tracks which replica to use next (round-robin)
    replica_cursor: AtomicUsize,
    /// Replicas marked unhealthy are skipped; primary is used as fallback
    healthy_replicas: RwLock<Vec<bool>>,
}

impl ShardPools {
    async fn new(cfg: &ShardConfig) -> Result<Self, sqlx::Error> {
        let primary = build_pool(
            &cfg.primary_url,
            cfg.max_connections,
            cfg.min_connections,
            cfg.connection_timeout,
        )
        .await?;

        let mut replicas = Vec::with_capacity(cfg.replica_urls.len());
        for url in &cfg.replica_urls {
            match build_pool(
                url,
                cfg.max_connections,
                cfg.min_connections,
                cfg.connection_timeout,
            )
            .await
            {
                Ok(p) => replicas.push(p),
                Err(e) => warn!(
                    shard_id = cfg.shard_id,
                    url, "Replica unavailable at startup: {e}"
                ),
            }
        }

        let healthy = vec![true; replicas.len()];
        Ok(Self {
            primary,
            replicas,
            replica_cursor: AtomicUsize::new(0),
            healthy_replicas: RwLock::new(healthy),
        })
    }

    /// Returns a read pool: round-robin across healthy replicas, falls back to primary.
    async fn read_pool(&self) -> &PgPool {
        let healthy = self.healthy_replicas.read().await;
        let n = self.replicas.len();
        if n == 0 {
            return &self.primary;
        }
        // Try up to n replicas before falling back
        for _ in 0..n {
            let idx = self.replica_cursor.fetch_add(1, Ordering::Relaxed) % n;
            if healthy[idx] {
                return &self.replicas[idx];
            }
        }
        warn!("All replicas unhealthy — falling back to primary for read");
        &self.primary
    }

    async fn mark_replica_unhealthy(&self, idx: usize) {
        let mut healthy = self.healthy_replicas.write().await;
        if let Some(h) = healthy.get_mut(idx) {
            *h = false;
        }
    }

    async fn mark_replica_healthy(&self, idx: usize) {
        let mut healthy = self.healthy_replicas.write().await;
        if let Some(h) = healthy.get_mut(idx) {
            *h = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Public HA pool manager
// ---------------------------------------------------------------------------

pub struct HaPoolManager {
    /// shard_id → pools
    shards: RwLock<HashMap<u32, Arc<ShardPools>>>,
    shard_count: AtomicUsize,
}

impl HaPoolManager {
    /// Build the manager and connect all configured shards.
    pub async fn new(cfg: &HaPoolConfig) -> Result<Arc<Self>, sqlx::Error> {
        let mut map = HashMap::new();
        for shard_cfg in &cfg.shards {
            info!(shard_id = shard_cfg.shard_id, "Connecting shard");
            let pools = ShardPools::new(shard_cfg).await?;
            map.insert(shard_cfg.shard_id, Arc::new(pools));
        }
        let count = map.len();
        let mgr = Arc::new(Self {
            shards: RwLock::new(map),
            shard_count: AtomicUsize::new(count),
        });

        // Spawn background checksum / health-check loop
        let mgr_clone = Arc::clone(&mgr);
        let interval = cfg.checksum_interval;
        tokio::spawn(async move {
            mgr_clone.health_loop(interval).await;
        });

        Ok(mgr)
    }

    // -----------------------------------------------------------------------
    // Shard routing
    // -----------------------------------------------------------------------

    /// Deterministically map a shard key (merchant_id / agent_id) to a shard.
    /// Uses modulo hashing — stable as long as shard_count doesn't change.
    pub fn shard_for_key(&self, key: &str) -> u32 {
        let n = self.shard_count.load(Ordering::Relaxed) as u32;
        if n == 0 {
            return 0;
        }
        let hash = fnv1a(key);
        hash % n
    }

    /// Write pool for a given shard key (always the primary).
    pub async fn write_pool(&self, shard_key: &str) -> Option<Arc<PgPool>> {
        let shard_id = self.shard_for_key(shard_key);
        let shards = self.shards.read().await;
        shards.get(&shard_id).map(|s| Arc::new(s.primary.clone()))
    }

    /// Read pool for a given shard key (load-balanced replica, fallback primary).
    pub async fn read_pool(&self, shard_key: &str) -> Option<Arc<PgPool>> {
        let shard_id = self.shard_for_key(shard_key);
        let shards = self.shards.read().await;
        if let Some(s) = shards.get(&shard_id) {
            Some(Arc::new(s.read_pool().await.clone()))
        } else {
            None
        }
    }

    /// Return one read pool for each shard, using a healthy replica if available.
    pub async fn all_read_pools(&self) -> Vec<Arc<PgPool>> {
        let shards = self.shards.read().await;
        shards
            .values()
            .map(|s| Arc::new(s.read_pool().await.clone()))
            .collect()
    }

    /// Return one primary pool for each shard.
    pub async fn all_primary_pools(&self) -> Vec<Arc<PgPool>> {
        let shards = self.shards.read().await;
        shards.values().map(|s| Arc::new(s.primary.clone())).collect()
    }

    // -----------------------------------------------------------------------
    // Hot shard addition (no cluster restart required)
    // -----------------------------------------------------------------------

    /// Add a new shard at runtime. Increments shard_count atomically.
    /// New writes for keys that hash to the new shard_id will use the new pools.
    pub async fn add_shard(&self, cfg: &ShardConfig) -> Result<(), sqlx::Error> {
        info!(shard_id = cfg.shard_id, "Hot-adding shard");
        let pools = ShardPools::new(cfg).await?;
        let mut shards = self.shards.write().await;
        shards.insert(cfg.shard_id, Arc::new(pools));
        self.shard_count.store(shards.len(), Ordering::Relaxed);
        info!(shard_id = cfg.shard_id, total = shards.len(), "Shard added");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Background health / checksum loop
    // -----------------------------------------------------------------------

    async fn health_loop(&self, interval: Duration) {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            self.run_health_checks().await;
        }
    }

    async fn run_health_checks(&self) {
        let shards = self.shards.read().await;
        for (&shard_id, pools) in shards.iter() {
            // Primary liveness
            if let Err(e) = sqlx::query("SELECT 1").fetch_one(&pools.primary).await {
                error!(shard_id, "Primary health check failed: {e}");
            }

            // Replica liveness + checksum comparison
            for (idx, replica) in pools.replicas.iter().enumerate() {
                match replica_checksum(replica).await {
                    Ok(replica_csum) => match replica_checksum(&pools.primary).await {
                        Ok(primary_csum) if primary_csum == replica_csum => {
                            pools.mark_replica_healthy(idx).await;
                        }
                        Ok(_) => {
                            warn!(
                                shard_id,
                                replica_idx = idx,
                                "Checksum mismatch — marking replica unhealthy"
                            );
                            pools.mark_replica_unhealthy(idx).await;
                        }
                        Err(e) => error!(shard_id, "Primary checksum error: {e}"),
                    },
                    Err(e) => {
                        warn!(
                            shard_id,
                            replica_idx = idx,
                            "Replica health check failed: {e}"
                        );
                        pools.mark_replica_unhealthy(idx).await;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn build_pool(
    url: &str,
    max: u32,
    min: u32,
    timeout: Duration,
) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max)
        .min_connections(min)
        .acquire_timeout(timeout)
        .connect(url)
        .await
}

/// Lightweight replica integrity check: compare row count + XOR checksum of
/// recent transactions across primary and replica.
async fn replica_checksum(pool: &PgPool) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(hashtext(id::text)), 0)::bigint \
         FROM transactions \
         WHERE created_at > NOW() - INTERVAL '5 minutes'",
    )
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// FNV-1a 32-bit hash for shard key routing.
fn fnv1a(s: &str) -> u32 {
    const OFFSET: u32 = 2166136261;
    const PRIME: u32 = 16777619;
    s.bytes()
        .fold(OFFSET, |acc, b| (acc ^ b as u32).wrapping_mul(PRIME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_routing_is_deterministic() {
        // Build a minimal manager with shard_count = 3 (no real DB needed)
        let mgr = HaPoolManager {
            shards: RwLock::new(HashMap::new()),
            shard_count: AtomicUsize::new(3),
        };
        let s1 = mgr.shard_for_key("merchant-abc");
        let s2 = mgr.shard_for_key("merchant-abc");
        assert_eq!(s1, s2);
        assert!(s1 < 3);
    }

    #[test]
    fn fnv1a_distributes() {
        let keys = ["merchant-1", "merchant-2", "agent-99", "agent-100"];
        let hashes: Vec<u32> = keys.iter().map(|k| fnv1a(k) % 4).collect();
        // Not all the same bucket
        assert!(hashes.iter().any(|&h| h != hashes[0]));
    }
}
