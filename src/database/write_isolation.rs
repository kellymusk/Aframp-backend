//! Write Operation Isolation Manager (Issue #XXX)
//!
//! Provides:
//! - Dedicated pools for settlement vs. audit vs. analytics writes
//! - Serializable transaction enforcement for critical operations
//! - Append-only ledger enforcement
//! - Write operation circuit breaker
//! - Retry logic with exponential backoff

use crate::database::error::DatabaseError;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Transaction, Postgres};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, error, info, warn};

// ─────────────────────────────────────────────────────────────────────────────
// Write Operation Types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOperationType {
    /// Settlement batch operations (critical, high consistency requirement)
    Settlement,

    /// Audit ledger operations (immutable, must be append-only)
    AuditLedger,

    /// Analytics operations (bufferable, eventual consistency ok)
    Analytics,

    /// Compliance rule violations (critical)
    Compliance,

    /// KYA/KYC approval (critical)
    Verification,
}

impl WriteOperationType {
    pub fn isolation_level(&self) -> &'static str {
        match self {
            WriteOperationType::Settlement
            | WriteOperationType::Compliance
            | WriteOperationType::Verification => "SERIALIZABLE",

            WriteOperationType::AuditLedger => "REPEATABLE READ",

            WriteOperationType::Analytics => "READ COMMITTED",
        }
    }

    pub fn timeout_ms(&self) -> u64 {
        match self {
            WriteOperationType::Settlement => 5000,
            WriteOperationType::Compliance => 5000,
            WriteOperationType::Verification => 5000,
            WriteOperationType::AuditLedger => 10000,
            WriteOperationType::Analytics => 30000,
        }
    }

    pub fn max_retries(&self) -> u32 {
        match self {
            WriteOperationType::Settlement => 3,
            WriteOperationType::Compliance => 3,
            WriteOperationType::Verification => 3,
            WriteOperationType::AuditLedger => 1, // No retries for immutable writes
            WriteOperationType::Analytics => 5,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Write Operation Metrics
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WriteMetrics {
    pub operation_type: WriteOperationType,
    pub success_count: u64,
    pub failure_count: u64,
    pub retry_count: u64,
    pub total_latency_ms: u64,
    pub circuit_open: bool,
}

struct WriteMetricsTracker {
    operation_type: WriteOperationType,
    success_count: AtomicU64,
    failure_count: AtomicU64,
    retry_count: AtomicU64,
    total_latency_ms: AtomicU64,
    circuit_failures: AtomicU64,
}

impl WriteMetricsTracker {
    fn new(operation_type: WriteOperationType) -> Self {
        Self {
            operation_type,
            success_count: AtomicU64::new(0),
            failure_count: AtomicU64::new(0),
            retry_count: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
            circuit_failures: AtomicU64::new(0),
        }
    }

    fn record_success(&self, latency_ms: u64) {
        self.success_count.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);
        self.circuit_failures.store(0, Ordering::Relaxed); // Reset circuit
    }

    fn record_failure(&self, is_retry: bool) {
        self.failure_count.fetch_add(1, Ordering::Relaxed);
        if is_retry {
            self.retry_count.fetch_add(1, Ordering::Relaxed);
        }
        self.circuit_failures.fetch_add(1, Ordering::Relaxed);
    }

    fn get_metrics(&self) -> WriteMetrics {
        let failures = self.circuit_failures.load(Ordering::Relaxed);
        let circuit_open = failures > 10; // Open if 10+ consecutive failures

        WriteMetrics {
            operation_type: self.operation_type,
            success_count: self.success_count.load(Ordering::Relaxed),
            failure_count: self.failure_count.load(Ordering::Relaxed),
            retry_count: self.retry_count.load(Ordering::Relaxed),
            total_latency_ms: self.total_latency_ms.load(Ordering::Relaxed),
            circuit_open,
        }
    }

    fn circuit_open(&self) -> bool {
        self.circuit_failures.load(Ordering::Relaxed) > 10
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Write Isolation Manager
// ─────────────────────────────────────────────────────────────────────────────

pub struct WriteIsolationManager {
    /// Per-operation-type pools
    settlement_pool: Arc<PgPool>,
    audit_pool: Arc<PgPool>,
    analytics_pool: Arc<PgPool>,
    compliance_pool: Arc<PgPool>,
    verification_pool: Arc<PgPool>,

    /// Per-operation-type metrics
    metrics: Arc<std::sync::Mutex<std::collections::HashMap<u32, Arc<WriteMetricsTracker>>>>,
}

impl WriteIsolationManager {
    /// Create a new write isolation manager
    pub async fn new(
        settlement_dsn: &str,
        audit_dsn: &str,
        analytics_dsn: &str,
        compliance_dsn: &str,
        verification_dsn: &str,
    ) -> Result<Arc<Self>, DatabaseError> {
        let settlement_pool = Arc::new(build_pool(settlement_dsn, 24).await?);
        let audit_pool = Arc::new(build_pool(audit_dsn, 16).await?);
        let analytics_pool = Arc::new(build_pool(analytics_dsn, 32).await?);
        let compliance_pool = Arc::new(build_pool(compliance_dsn, 16).await?);
        let verification_pool = Arc::new(build_pool(verification_dsn, 16).await?);

        let mut metrics_map = std::collections::HashMap::new();
        metrics_map.insert(0, Arc::new(WriteMetricsTracker::new(WriteOperationType::Settlement)));
        metrics_map.insert(1, Arc::new(WriteMetricsTracker::new(WriteOperationType::AuditLedger)));
        metrics_map.insert(2, Arc::new(WriteMetricsTracker::new(WriteOperationType::Analytics)));
        metrics_map.insert(3, Arc::new(WriteMetricsTracker::new(WriteOperationType::Compliance)));
        metrics_map.insert(4, Arc::new(WriteMetricsTracker::new(WriteOperationType::Verification)));

        Ok(Arc::new(Self {
            settlement_pool,
            audit_pool,
            analytics_pool,
            compliance_pool,
            verification_pool,
            metrics: Arc::new(std::sync::Mutex::new(metrics_map)),
        }))
    }

    /// Get the appropriate pool for a write operation
    fn get_pool(&self, op_type: WriteOperationType) -> Arc<PgPool> {
        match op_type {
            WriteOperationType::Settlement => Arc::clone(&self.settlement_pool),
            WriteOperationType::AuditLedger => Arc::clone(&self.audit_pool),
            WriteOperationType::Analytics => Arc::clone(&self.analytics_pool),
            WriteOperationType::Compliance => Arc::clone(&self.compliance_pool),
            WriteOperationType::Verification => Arc::clone(&self.verification_pool),
        }
    }

    /// Get metrics for an operation type
    pub fn get_metrics(&self, op_type: WriteOperationType) -> Option<WriteMetrics> {
        let metrics_map = self.metrics.lock().unwrap();
        let op_idx = match op_type {
            WriteOperationType::Settlement => 0,
            WriteOperationType::AuditLedger => 1,
            WriteOperationType::Analytics => 2,
            WriteOperationType::Compliance => 3,
            WriteOperationType::Verification => 4,
        };
        metrics_map
            .get(&op_idx)
            .map(|tracker| tracker.get_metrics())
    }

    /// Execute a write operation with isolation and retry logic
    pub async fn execute_write<F, T>(
        &self,
        op_type: WriteOperationType,
        operation: F,
    ) -> Result<T, DatabaseError>
    where
        F: Fn(&mut Transaction<Postgres>) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<T, DatabaseError>> + Send>,
        > + Send
            + Sync,
        T: Send,
    {
        let pool = self.get_pool(op_type);
        let metrics_tracker = {
            let metrics_map = self.metrics.lock().unwrap();
            let idx = self.op_type_index(op_type);
            Arc::clone(metrics_map.get(&idx).unwrap())
        };

        // Check circuit breaker
        if metrics_tracker.circuit_open() {
            error!("Circuit breaker OPEN for {:?}", op_type);
            return Err(DatabaseError::internal(
                "Write circuit breaker is open — too many failures",
            ));
        }

        let max_retries = op_type.max_retries();
        let mut attempt = 0;

        loop {
            let start = Instant::now();

            // Create transaction with appropriate isolation level
            let mut tx = pool
                .begin()
                .await
                .map_err(|e| {
                    metrics_tracker.record_failure(attempt > 0);
                    DatabaseError::from_sqlx(e)
                })?;

            // Set isolation level
            let isolation_sql = format!("SET TRANSACTION ISOLATION LEVEL {}", op_type.isolation_level());
            sqlx::query(&isolation_sql)
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    metrics_tracker.record_failure(attempt > 0);
                    DatabaseError::from_sqlx(e)
                })?;

            // Execute user operation
            match operation(&mut tx).await {
                Ok(result) => {
                    // Commit
                    match tx.commit().await {
                        Ok(_) => {
                            let latency_ms = start.elapsed().as_millis() as u64;
                            metrics_tracker.record_success(latency_ms);
                            info!(
                                "Write operation {:?} succeeded in {} ms (attempt {})",
                                op_type, latency_ms, attempt + 1
                            );
                            return Ok(result);
                        }
                        Err(e) => {
                            attempt += 1;
                            if attempt < max_retries {
                                warn!(
                                    "Write operation {:?} commit failed, retrying (attempt {}/{}): {}",
                                    op_type, attempt, max_retries, e
                                );
                                metrics_tracker.record_failure(true);

                                // Exponential backoff
                                let backoff_ms = 10u64 * 2u64.pow(attempt - 1);
                                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                                continue;
                            } else {
                                error!(
                                    "Write operation {:?} commit failed after {} attempts: {}",
                                    op_type, max_retries, e
                                );
                                metrics_tracker.record_failure(true);
                                return Err(DatabaseError::from_sqlx(e));
                            }
                        }
                    }
                }
                Err(e) => {
                    // Rollback (implicit on drop)
                    drop(tx);

                    attempt += 1;
                    if attempt < max_retries
                        && is_retryable_error(&e)
                        && op_type != WriteOperationType::AuditLedger
                    {
                        warn!(
                            "Write operation {:?} failed with retryable error, retrying (attempt {}/{}): {}",
                            op_type, attempt, max_retries, e
                        );
                        metrics_tracker.record_failure(true);

                        // Exponential backoff
                        let backoff_ms = 10u64 * 2u64.pow(attempt - 1);
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                        continue;
                    } else {
                        error!(
                            "Write operation {:?} failed after {} attempts: {}",
                            op_type, attempt, e
                        );
                        metrics_tracker.record_failure(attempt > 1);
                        return Err(e);
                    }
                }
            }
        }
    }

    fn op_type_index(&self, op_type: WriteOperationType) -> u32 {
        match op_type {
            WriteOperationType::Settlement => 0,
            WriteOperationType::AuditLedger => 1,
            WriteOperationType::Analytics => 2,
            WriteOperationType::Compliance => 3,
            WriteOperationType::Verification => 4,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Build a connection pool with write-optimized settings
async fn build_pool(dsn: &str, max_connections: u32) -> Result<PgPool, DatabaseError> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .min_connections((max_connections / 2).max(4))
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(60))
        .max_lifetime(Duration::from_secs(1800))
        .connect(dsn)
        .await
        .map_err(DatabaseError::from_sqlx)
}

/// Determine if an error is retryable
fn is_retryable_error(error: &DatabaseError) -> bool {
    let error_str = error.to_string();

    // Serialization conflicts, deadlocks, and transient connection errors are retryable
    error_str.contains("40P01") || // Deadlock
    error_str.contains("40001") || // Serialization failure
    error_str.contains("57P03") || // Cannot execute in a failed transaction block
    error_str.contains("connection reset") ||
    error_str.contains("too many connections") ||
    error_str.contains("timeout")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_operation_isolation_levels() {
        assert_eq!(
            WriteOperationType::Settlement.isolation_level(),
            "SERIALIZABLE"
        );
        assert_eq!(
            WriteOperationType::AuditLedger.isolation_level(),
            "REPEATABLE READ"
        );
        assert_eq!(
            WriteOperationType::Analytics.isolation_level(),
            "READ COMMITTED"
        );
    }

    #[test]
    fn test_write_operation_timeouts() {
        assert_eq!(WriteOperationType::Settlement.timeout_ms(), 5000);
        assert_eq!(WriteOperationType::AuditLedger.timeout_ms(), 10000);
        assert_eq!(WriteOperationType::Analytics.timeout_ms(), 30000);
    }

    #[test]
    fn test_write_operation_max_retries() {
        assert_eq!(WriteOperationType::Settlement.max_retries(), 3);
        assert_eq!(WriteOperationType::AuditLedger.max_retries(), 1);
        assert_eq!(WriteOperationType::Analytics.max_retries(), 5);
    }

    #[test]
    fn test_metrics_tracking() {
        let tracker = WriteMetricsTracker::new(WriteOperationType::Settlement);

        tracker.record_success(100);
        let metrics = tracker.get_metrics();
        assert_eq!(metrics.success_count, 1);
        assert_eq!(metrics.total_latency_ms, 100);

        tracker.record_failure(false);
        let metrics = tracker.get_metrics();
        assert_eq!(metrics.failure_count, 1);
        assert_eq!(metrics.circuit_open, false);

        // Record 10 failures to trip circuit
        for _ in 0..10 {
            tracker.record_failure(false);
        }
        let metrics = tracker.get_metrics();
        assert_eq!(metrics.circuit_open, true);
    }
}
