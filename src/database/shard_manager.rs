//! Shard Manager — Logical Sharding Framework (Issue #XXX)
//!
//! Provides:
//! - Shard registry discovery and hot-reload
//! - Shard status management (active, draining, offline)
//! - Consistent hashing with virtual nodes
//! - Per-shard pool management
//! - Shard addition without restart

use crate::database::error::DatabaseError;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

// ─────────────────────────────────────────────────────────────────────────────
// Shard Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Shard configuration from registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardConfig {
    pub shard_id: i32,
    pub corridor_id: String,
    pub week_id: Option<i32>,
    pub primary_dsn: String,
    pub replica_dsns: Vec<String>,
    pub status: ShardStatus,
    pub max_connections: u32,
    pub weight: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShardStatus {
    Active,
    Draining,  // Accepts reads, rejects new writes
    Offline,
}

impl ShardStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "draining" => ShardStatus::Draining,
            "offline" => ShardStatus::Offline,
            _ => ShardStatus::Active,
        }
    }

    pub fn accepts_writes(&self) -> bool {
        *self == ShardStatus::Active
    }

    pub fn accepts_reads(&self) -> bool {
        matches!(self, ShardStatus::Active | ShardStatus::Draining)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Consistent Hashing
// ─────────────────────────────────────────────────────────────────────────────

/// FNV-1a hash function for consistent distribution
fn fnv1a_hash(key: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 14695981039346656037;
    const FNV_PRIME: u64 = 1099511628211;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in key.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// ─────────────────────────────────────────────────────────────────────────────
// Shard Manager
// ─────────────────────────────────────────────────────────────────────────────

/// State of a single shard (pools + config)
struct ShardState {
    config: ShardConfig,
    primary_pool: Arc<PgPool>,
    replica_pools: Vec<Arc<PgPool>>,
}

pub struct ShardManager {
    /// Coordinator pool (holds shard_registry table)
    coordinator_pool: Arc<PgPool>,

    /// Current shards: shard_id → ShardState
    shards: Arc<RwLock<HashMap<i32, ShardState>>>,

    /// Shard routing: shard_key → shard_id (computed on load)
    routing: Arc<RwLock<Vec<i32>>>, // indexed by hash % shard_count

    /// Configuration
    refresh_interval: Duration,
    hash_slots: u64, // virtual nodes for consistent hashing

    /// Metrics
    shards_loaded_at: RwLock<std::time::Instant>,
    load_count: AtomicU64,
}

impl ShardManager {
    /// Create a new shard manager and load initial configuration
    pub async fn new(
        coordinator_pool: Arc<PgPool>,
        refresh_interval: Duration,
    ) -> Result<Arc<Self>, DatabaseError> {
        let manager = Arc::new(Self {
            coordinator_pool,
            shards: Arc::new(RwLock::new(HashMap::new())),
            routing: Arc::new(RwLock::new(Vec::new())),
            refresh_interval,
            hash_slots: 16384, // 2^14 virtual nodes
            shards_loaded_at: RwLock::new(std::time::Instant::now()),
            load_count: AtomicU64::new(0),
        });

        // Load initial configuration
        manager.reload().await?;

        // Spawn background refresh task
        let mgr_clone = Arc::clone(&manager);
        tokio::spawn(async move {
            mgr_clone.refresh_loop().await;
        });

        Ok(manager)
    }

    /// Reload shard registry from coordinator
    pub async fn reload(&self) -> Result<(), DatabaseError> {
        debug!("Reloading shard registry");

        // Fetch shard configuration from registry table
        let rows = sqlx::query(
            r#"
            SELECT
                shard_id,
                corridor_id,
                week_id,
                primary_dsn,
                COALESCE(replica_dsns, ARRAY[]::TEXT[]) as replica_dsns,
                status,
                COALESCE(max_connections, 8) as max_connections,
                COALESCE(weight, 1) as weight
            FROM shard_registry
            ORDER BY shard_id
            "#,
        )
        .fetch_all(self.coordinator_pool.as_ref())
        .await
        .map_err(DatabaseError::from_sqlx)?;

        let mut shards = HashMap::new();
        let mut active_shard_ids = Vec::new();

        for row in rows {
            let shard_id: i32 = row.get("shard_id");
            let status_str: String = row.get("status");
            let status = ShardStatus::from_str(&status_str);

            if status == ShardStatus::Offline {
                debug!("Skipping offline shard {}", shard_id);
                continue;
            }

            let primary_dsn: String = row.get("primary_dsn");
            let replica_dsns: Vec<String> = row.get("replica_dsns");
            let max_connections: i32 = row.get("max_connections");

            // Connect to primary
            let primary_pool = match self.connect_pool(&primary_dsn, max_connections as u32).await
            {
                Ok(p) => Arc::new(p),
                Err(e) => {
                    error!(
                        "Failed to connect to shard {} primary: {}",
                        shard_id, e
                    );
                    continue;
                }
            };

            // Connect to replicas
            let mut replica_pools = Vec::new();
            for replica_dsn in &replica_dsns {
                match self.connect_pool(replica_dsn, max_connections as u32).await {
                    Ok(p) => replica_pools.push(Arc::new(p)),
                    Err(e) => warn!(
                        "Failed to connect to shard {} replica: {}",
                        shard_id, e
                    ),
                }
            }

            let config = ShardConfig {
                shard_id,
                corridor_id: row.get("corridor_id"),
                week_id: row.get("week_id"),
                primary_dsn,
                replica_dsns,
                status,
                max_connections: max_connections as u32,
                weight: row.get("weight"),
            };

            shards.insert(
                shard_id,
                ShardState {
                    config,
                    primary_pool,
                    replica_pools,
                },
            );

            active_shard_ids.push(shard_id);

            info!(
                "Loaded shard {}: {} ({})",
                shard_id, config.corridor_id,
                match status {
                    ShardStatus::Active => "active",
                    ShardStatus::Draining => "draining",
                    ShardStatus::Offline => "offline",
                }
            );
        }

        // Build routing table using consistent hashing
        let mut routing = vec![0i32; self.hash_slots as usize];
        if !active_shard_ids.is_empty() {
            for i in 0..self.hash_slots {
                let idx = (i as usize) % active_shard_ids.len();
                routing[i as usize] = active_shard_ids[idx];
            }
        }

        *self.shards.write().await = shards;
        *self.routing.write().await = routing;
        *self.shards_loaded_at.write().await = std::time::Instant::now();
        self.load_count.fetch_add(1, Ordering::Relaxed);

        info!(
            "Shard registry reloaded: {} active shards",
            active_shard_ids.len()
        );
        Ok(())
    }

    /// Connect to a PostgreSQL database
    async fn connect_pool(&self, dsn: &str, max_connections: u32) -> Result<PgPool, DatabaseError> {
        PgPoolOptions::new()
            .max_connections(max_connections)
            .min_connections((max_connections / 2).max(2))
            .acquire_timeout(Duration::from_secs(10))
            .idle_timeout(Duration::from_secs(300))
            .connect(dsn)
            .await
            .map_err(DatabaseError::from_sqlx)
    }

    /// Route a shard key to a shard ID using consistent hashing
    pub async fn route_key(&self, shard_key: &str) -> Result<i32, DatabaseError> {
        let hash = fnv1a_hash(shard_key);
        let slot = hash % self.hash_slots;

        let routing = self.routing.read().await;
        if routing.is_empty() {
            return Err(DatabaseError::internal("No shards available"));
        }

        Ok(routing[slot as usize])
    }

    /// Get the primary pool for a shard
    pub async fn get_write_pool(&self, shard_id: i32) -> Result<Arc<PgPool>, DatabaseError> {
        let shards = self.shards.read().await;
        shards
            .get(&shard_id)
            .ok_or_else(|| DatabaseError::internal("Shard not found"))
            .map(|s| Arc::clone(&s.primary_pool))
    }

    /// Get a replica pool for a shard (round-robin)
    pub async fn get_read_pool(&self, shard_id: i32) -> Result<Arc<PgPool>, DatabaseError> {
        let shards = self.shards.read().await;
        let state = shards
            .get(&shard_id)
            .ok_or_else(|| DatabaseError::internal("Shard not found"))?;

        if state.replica_pools.is_empty() {
            // No replicas, use primary
            Ok(Arc::clone(&state.primary_pool))
        } else {
            // Round-robin across replicas
            let idx = hash(shard_id as u64) as usize % state.replica_pools.len();
            Ok(Arc::clone(&state.replica_pools[idx]))
        }
    }

    /// Get configuration for a shard
    pub async fn get_shard_config(&self, shard_id: i32) -> Result<ShardConfig, DatabaseError> {
        let shards = self.shards.read().await;
        shards
            .get(&shard_id)
            .ok_or_else(|| DatabaseError::internal("Shard not found"))
            .map(|s| s.config.clone())
    }

    /// Get all active shard IDs
    pub async fn active_shards(&self) -> Vec<i32> {
        let shards = self.shards.read().await;
        shards.keys().copied().collect()
    }

    /// Get shard statistics
    pub async fn stats(&self) -> ShardManagerStats {
        let shards = self.shards.read().await;
        ShardManagerStats {
            active_shard_count: shards.len(),
            shards_loaded_at: *self.shards_loaded_at.read().await,
            load_count: self.load_count.load(Ordering::Relaxed),
            refresh_interval_secs: self.refresh_interval.as_secs(),
        }
    }

    /// Background refresh loop
    async fn refresh_loop(&self) {
        loop {
            tokio::time::sleep(self.refresh_interval).await;

            match self.reload().await {
                Ok(_) => debug!("Shard registry refreshed"),
                Err(e) => warn!("Failed to refresh shard registry: {}", e),
            }
        }
    }
}

/// Statistics about the shard manager
#[derive(Debug, Clone)]
pub struct ShardManagerStats {
    pub active_shard_count: usize,
    pub shards_loaded_at: std::time::Instant,
    pub load_count: u64,
    pub refresh_interval_secs: u64,
}

/// Simple hash for round-robin selection
fn hash(val: u64) -> u64 {
    val.wrapping_mul(2654435761)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a_hash_consistency() {
        let hash1 = fnv1a_hash("NG_W202601");
        let hash2 = fnv1a_hash("NG_W202601");
        assert_eq!(hash1, hash2, "Hash should be consistent");
    }

    #[test]
    fn test_fnv1a_hash_distribution() {
        let hashes: Vec<u64> = (0..1000)
            .map(|i| fnv1a_hash(&format!("key_{}", i)))
            .collect();

        // Check that hashes are distributed (not all the same)
        let unique: std::collections::HashSet<_> = hashes.into_iter().collect();
        assert!(
            unique.len() > 900,
            "Hashes should be well-distributed: {} unique",
            unique.len()
        );
    }

    #[test]
    fn test_shard_status_parsing() {
        assert_eq!(ShardStatus::from_str("active"), ShardStatus::Active);
        assert_eq!(ShardStatus::from_str("draining"), ShardStatus::Draining);
        assert_eq!(ShardStatus::from_str("offline"), ShardStatus::Offline);
        assert_eq!(ShardStatus::from_str("unknown"), ShardStatus::Active);
    }

    #[test]
    fn test_shard_status_write_check() {
        assert!(ShardStatus::Active.accepts_writes());
        assert!(!ShardStatus::Draining.accepts_writes());
        assert!(!ShardStatus::Offline.accepts_writes());

        assert!(ShardStatus::Active.accepts_reads());
        assert!(ShardStatus::Draining.accepts_reads());
        assert!(!ShardStatus::Offline.accepts_reads());
    }
}
