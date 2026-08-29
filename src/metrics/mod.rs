//! Prometheus metrics registry and all metric definitions for Aframp backend.
//!
//! All metrics are registered in a single global registry exposed at GET /metrics.
//! Metric names follow Prometheus naming conventions: snake_case, unit suffix where
//! applicable, and the `aframp_` namespace prefix.
pub mod analytics;
pub mod geo_restriction;
pub mod handler;
pub mod issuer;
pub mod por;
pub mod tests;

use prometheus::{
    register_counter_vec_with_registry, register_gauge_vec_with_registry,
    register_histogram_vec_with_registry, CounterVec, GaugeVec, HistogramVec, Registry,
};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Global registry
// ---------------------------------------------------------------------------

static REGISTRY: OnceLock<Registry> = OnceLock::new();

/// Returns the global Prometheus registry, initialising it on first call.
pub fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| {
        let r = Registry::new();
        register_all(&r);
        r
    })
}

/// Render all metrics in Prometheus text exposition format.
pub fn render() -> String {
    use prometheus::Encoder;
    let encoder = prometheus::TextEncoder::new();
    let mut buf = Vec::new();
    encoder
        .encode(&registry().gather(), &mut buf)
        .expect("encoding metrics failed");
    String::from_utf8(buf).expect("metrics output is not valid UTF-8")
}

// ---------------------------------------------------------------------------
// HTTP request metrics
// ---------------------------------------------------------------------------

pub mod http {
    use super::*;

    static HTTP_REQUESTS_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static HTTP_REQUEST_DURATION_SECONDS: OnceLock<HistogramVec> = OnceLock::new();
    static HTTP_REQUESTS_IN_FLIGHT: OnceLock<GaugeVec> = OnceLock::new();

    pub fn requests_total() -> &'static CounterVec {
        HTTP_REQUESTS_TOTAL.get().expect("metrics not initialised")
    }

    pub fn request_duration_seconds() -> &'static HistogramVec {
        HTTP_REQUEST_DURATION_SECONDS
            .get()
            .expect("metrics not initialised")
    }

    pub fn requests_in_flight() -> &'static GaugeVec {
        HTTP_REQUESTS_IN_FLIGHT
            .get()
            .expect("metrics not initialised")
    }

    pub(super) fn register(r: &Registry) {
        HTTP_REQUESTS_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_http_requests_total",
                    "Total number of HTTP requests",
                    &["method", "route", "status_code"],
                    r
                )
                .unwrap(),
            )
            .ok();

        HTTP_REQUEST_DURATION_SECONDS
            .set(
                register_histogram_vec_with_registry!(
                    "aframp_http_request_duration_seconds",
                    "HTTP request duration in seconds",
                    &["method", "route"],
                    vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0],
                    r
                )
                .unwrap(),
            )
            .ok();

        HTTP_REQUESTS_IN_FLIGHT
            .set(
                register_gauge_vec_with_registry!(
                    "aframp_http_requests_in_flight",
                    "Number of HTTP requests currently being processed",
                    &["route"],
                    r
                )
                .unwrap(),
            )
            .ok();
    }
}

// ---------------------------------------------------------------------------
// Service authentication metrics
// ---------------------------------------------------------------------------

pub mod service_auth {
    use super::*;

    static SERVICE_TOKEN_ACQUISITIONS: OnceLock<CounterVec> = OnceLock::new();
    static SERVICE_TOKEN_REFRESH_EVENTS: OnceLock<CounterVec> = OnceLock::new();
    static SERVICE_TOKEN_REFRESH_FAILURES: OnceLock<CounterVec> = OnceLock::new();
    static SERVICE_CALL_AUTHENTICATIONS: OnceLock<CounterVec> = OnceLock::new();
    static SERVICE_CALL_AUTHORIZATION_DENIALS: OnceLock<CounterVec> = OnceLock::new();

    pub fn token_acquisitions() -> &'static CounterVec {
        SERVICE_TOKEN_ACQUISITIONS
            .get()
            .expect("metrics not initialised")
    }

    pub fn token_refresh_events() -> &'static CounterVec {
        SERVICE_TOKEN_REFRESH_EVENTS
            .get()
            .expect("metrics not initialised")
    }

    pub fn token_refresh_failures() -> &'static CounterVec {
        SERVICE_TOKEN_REFRESH_FAILURES
            .get()
            .expect("metrics not initialised")
    }

    pub fn service_call_authentications() -> &'static CounterVec {
        SERVICE_CALL_AUTHENTICATIONS
            .get()
            .expect("metrics not initialised")
    }

    pub fn service_call_authorization_denials() -> &'static CounterVec {
        SERVICE_CALL_AUTHORIZATION_DENIALS
            .get()
            .expect("metrics not initialised")
    }

    pub(super) fn register(r: &Registry) {
        SERVICE_TOKEN_ACQUISITIONS
            .set(
                register_counter_vec_with_registry!(
                    "aframp_service_token_acquisitions_total",
                    "Total service token acquisitions by service",
                    &["service_name"],
                    r
                )
                .unwrap(),
            )
            .ok();

        SERVICE_TOKEN_REFRESH_EVENTS
            .set(
                register_counter_vec_with_registry!(
                    "aframp_service_token_refresh_events_total",
                    "Total service token refresh events by service",
                    &["service_name"],
                    r
                )
                .unwrap(),
            )
            .ok();

        SERVICE_TOKEN_REFRESH_FAILURES
            .set(
                register_counter_vec_with_registry!(
                    "aframp_service_token_refresh_failures_total",
                    "Total service token refresh failures by service",
                    &["service_name"],
                    r
                )
                .unwrap(),
            )
            .ok();

        SERVICE_CALL_AUTHENTICATIONS
            .set(
                register_counter_vec_with_registry!(
                    "aframp_service_call_authentications_total",
                    "Total service call authentications by calling service, endpoint, and result",
                    &["calling_service", "endpoint", "result"],
                    r
                )
                .unwrap(),
            )
            .ok();

        SERVICE_CALL_AUTHORIZATION_DENIALS
            .set(
                register_counter_vec_with_registry!(
                    "aframp_service_call_authorization_denials_total",
                    "Total service call authorization denials by calling service, endpoint, and reason",
                    &["calling_service", "endpoint", "reason"],
                    r
                )
                .unwrap(),
            )
            .ok();
    }
}

// ---------------------------------------------------------------------------
// cNGN transaction metrics
// ---------------------------------------------------------------------------

pub mod cngn {
    use super::*;

    static CNGN_TRANSACTIONS_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static CNGN_TRANSACTION_VOLUME: OnceLock<HistogramVec> = OnceLock::new();
    static CNGN_TRANSACTION_DURATION_SECONDS: OnceLock<HistogramVec> = OnceLock::new();

    pub fn transactions_total() -> &'static CounterVec {
        CNGN_TRANSACTIONS_TOTAL
            .get()
            .expect("metrics not initialised")
    }

    pub fn transaction_volume() -> &'static HistogramVec {
        CNGN_TRANSACTION_VOLUME
            .get()
            .expect("metrics not initialised")
    }

    pub fn transaction_duration_seconds() -> &'static HistogramVec {
        CNGN_TRANSACTION_DURATION_SECONDS
            .get()
            .expect("metrics not initialised")
    }

    pub(super) fn register(r: &Registry) {
        CNGN_TRANSACTIONS_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_cngn_transactions_total",
                    "Total cNGN transactions by type and status",
                    &["tx_type", "status"],
                    r
                )
                .unwrap(),
            )
            .ok();

        CNGN_TRANSACTION_VOLUME
            .set(
                register_histogram_vec_with_registry!(
                    "aframp_cngn_transaction_volume_ngn",
                    "cNGN transaction amounts in NGN",
                    &["tx_type"],
                    vec![
                        100.0,
                        500.0,
                        1_000.0,
                        5_000.0,
                        10_000.0,
                        50_000.0,
                        100_000.0,
                        500_000.0,
                        1_000_000.0,
                    ],
                    r
                )
                .unwrap(),
            )
            .ok();

        CNGN_TRANSACTION_DURATION_SECONDS
            .set(
                register_histogram_vec_with_registry!(
                    "aframp_cngn_transaction_duration_seconds",
                    "cNGN transaction processing duration from initiation to completion",
                    &["tx_type"],
                    vec![1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0],
                    r
                )
                .unwrap(),
            )
            .ok();
    }
}

// ---------------------------------------------------------------------------
// Payment provider metrics
// ---------------------------------------------------------------------------

pub mod payment {
    use super::*;

    static PAYMENT_PROVIDER_REQUESTS_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static PAYMENT_PROVIDER_REQUEST_DURATION_SECONDS: OnceLock<HistogramVec> = OnceLock::new();
    static PAYMENT_PROVIDER_FAILURES_TOTAL: OnceLock<CounterVec> = OnceLock::new();

    pub fn provider_requests_total() -> &'static CounterVec {
        PAYMENT_PROVIDER_REQUESTS_TOTAL
            .get()
            .expect("metrics not initialised")
    }

    pub fn provider_request_duration_seconds() -> &'static HistogramVec {
        PAYMENT_PROVIDER_REQUEST_DURATION_SECONDS
            .get()
            .expect("metrics not initialised")
    }

    pub fn provider_failures_total() -> &'static CounterVec {
        PAYMENT_PROVIDER_FAILURES_TOTAL
            .get()
            .expect("metrics not initialised")
    }

    pub(super) fn register(r: &Registry) {
        PAYMENT_PROVIDER_REQUESTS_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_payment_provider_requests_total",
                    "Total payment provider requests by provider and operation",
                    &["provider", "operation"],
                    r
                )
                .unwrap(),
            )
            .ok();

        PAYMENT_PROVIDER_REQUEST_DURATION_SECONDS
            .set(
                register_histogram_vec_with_registry!(
                    "aframp_payment_provider_request_duration_seconds",
                    "Payment provider request duration in seconds",
                    &["provider", "operation"],
                    vec![0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0],
                    r
                )
                .unwrap(),
            )
            .ok();

        PAYMENT_PROVIDER_FAILURES_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_payment_provider_failures_total",
                    "Total payment provider failures by provider and failure reason",
                    &["provider", "failure_reason"],
                    r
                )
                .unwrap(),
            )
            .ok();
    }
}

// ---------------------------------------------------------------------------
// Stellar service metrics
// ---------------------------------------------------------------------------

pub mod stellar {
    use super::*;

    static STELLAR_TX_SUBMISSIONS_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static STELLAR_TX_SUBMISSION_DURATION_SECONDS: OnceLock<HistogramVec> = OnceLock::new();
    static STELLAR_TRUSTLINE_ATTEMPTS_TOTAL: OnceLock<CounterVec> = OnceLock::new();

    pub fn tx_submissions_total() -> &'static CounterVec {
        STELLAR_TX_SUBMISSIONS_TOTAL
            .get()
            .expect("metrics not initialised")
    }

    pub fn tx_submission_duration_seconds() -> &'static HistogramVec {
        STELLAR_TX_SUBMISSION_DURATION_SECONDS
            .get()
            .expect("metrics not initialised")
    }

    pub fn trustline_attempts_total() -> &'static CounterVec {
        STELLAR_TRUSTLINE_ATTEMPTS_TOTAL
            .get()
            .expect("metrics not initialised")
    }

    pub(super) fn register(r: &Registry) {
        STELLAR_TX_SUBMISSIONS_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_stellar_tx_submissions_total",
                    "Total Stellar transaction submissions by status",
                    &["status"],
                    r
                )
                .unwrap(),
            )
            .ok();

        STELLAR_TX_SUBMISSION_DURATION_SECONDS
            .set(
                register_histogram_vec_with_registry!(
                    "aframp_stellar_tx_submission_duration_seconds",
                    "Stellar transaction submission duration in seconds",
                    &[],
                    vec![0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 30.0],
                    r
                )
                .unwrap(),
            )
            .ok();

        STELLAR_TRUSTLINE_ATTEMPTS_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_stellar_trustline_attempts_total",
                    "Total Stellar trustline creation attempts by status",
                    &["status"],
                    r
                )
                .unwrap(),
            )
            .ok();
    }
}

// ---------------------------------------------------------------------------
// Background worker metrics
// ---------------------------------------------------------------------------

pub mod worker {
    use super::*;

    static WORKER_CYCLES_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static WORKER_CYCLE_DURATION_SECONDS: OnceLock<HistogramVec> = OnceLock::new();
    static WORKER_RECORDS_PROCESSED: OnceLock<GaugeVec> = OnceLock::new();
    static WORKER_ERRORS_TOTAL: OnceLock<CounterVec> = OnceLock::new();

    pub fn cycles_total() -> &'static CounterVec {
        WORKER_CYCLES_TOTAL.get().expect("metrics not initialised")
    }

    pub fn cycle_duration_seconds() -> &'static HistogramVec {
        WORKER_CYCLE_DURATION_SECONDS
            .get()
            .expect("metrics not initialised")
    }

    pub fn records_processed() -> &'static GaugeVec {
        WORKER_RECORDS_PROCESSED
            .get()
            .expect("metrics not initialised")
    }

    pub fn errors_total() -> &'static CounterVec {
        WORKER_ERRORS_TOTAL.get().expect("metrics not initialised")
    }

    pub(super) fn register(r: &Registry) {
        WORKER_CYCLES_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_worker_cycles_total",
                    "Total background worker processing cycles",
                    &["worker"],
                    r
                )
                .unwrap(),
            )
            .ok();

        WORKER_CYCLE_DURATION_SECONDS
            .set(
                register_histogram_vec_with_registry!(
                    "aframp_worker_cycle_duration_seconds",
                    "Background worker cycle duration in seconds",
                    &["worker"],
                    vec![0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0],
                    r
                )
                .unwrap(),
            )
            .ok();

        WORKER_RECORDS_PROCESSED
            .set(
                register_gauge_vec_with_registry!(
                    "aframp_worker_records_processed",
                    "Number of records processed in the last worker cycle",
                    &["worker"],
                    r
                )
                .unwrap(),
            )
            .ok();

        WORKER_ERRORS_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_worker_errors_total",
                    "Total background worker errors by worker and error type",
                    &["worker", "error_type"],
                    r
                )
                .unwrap(),
            )
            .ok();
    }
}

// ---------------------------------------------------------------------------
// Redis cache metrics
// ---------------------------------------------------------------------------

pub mod cache {
    use super::*;

    static CACHE_HITS_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static CACHE_MISSES_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static CACHE_OPERATION_DURATION_SECONDS: OnceLock<HistogramVec> = OnceLock::new();
    static CACHE_HIT_RATIO_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static CDN_CACHE_STATUS_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static REDIS_MEMORY_USED_BYTES: OnceLock<GaugeVec> = OnceLock::new();
    static REDIS_MAXMEMORY_BYTES: OnceLock<GaugeVec> = OnceLock::new();

    pub fn redis_memory_used_bytes() -> &'static GaugeVec {
        REDIS_MEMORY_USED_BYTES.get().expect("metrics not initialised")
    }

    pub fn redis_maxmemory_bytes() -> &'static GaugeVec {
        REDIS_MAXMEMORY_BYTES.get().expect("metrics not initialised")
    }

    pub fn hits_total() -> &'static CounterVec {
        CACHE_HITS_TOTAL.get().expect("metrics not initialised")
    }

    pub fn misses_total() -> &'static CounterVec {
        CACHE_MISSES_TOTAL.get().expect("metrics not initialised")
    }

    pub fn operation_duration_seconds() -> &'static HistogramVec {
        CACHE_OPERATION_DURATION_SECONDS
            .get()
            .expect("metrics not initialised")
    }

    /// Per-tier, per-namespace hit counter. Compute ratio via `rate()` in Prometheus:
    ///   `rate(cache_hit_ratio_total{tier="l2",namespace="rate"}[5m])`
    pub fn cache_hit_ratio_total() -> &'static CounterVec {
        CACHE_HIT_RATIO_TOTAL.get().expect("metrics not initialised")
    }

    /// CDN edge cache status per path prefix.
    pub fn cdn_cache_status_total() -> &'static CounterVec {
        CDN_CACHE_STATUS_TOTAL.get().expect("metrics not initialised")
    }

    pub(super) fn register(r: &Registry) {
        CACHE_HITS_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_cache_hits_total",
                    "Total Redis cache hits by key prefix",
                    &["key_prefix"],
                    r
                )
                .unwrap(),
            )
            .ok();

        CACHE_MISSES_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_cache_misses_total",
                    "Total Redis cache misses by key prefix",
                    &["key_prefix"],
                    r
                )
                .unwrap(),
            )
            .ok();

        CACHE_OPERATION_DURATION_SECONDS
            .set(
                register_histogram_vec_with_registry!(
                    "aframp_cache_operation_duration_seconds",
                    "Redis cache operation duration in seconds",
                    &["operation"],
                    vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5],
                    r
                )
                .unwrap(),
            )
            .ok();

        CACHE_HIT_RATIO_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_cache_hit_ratio_total",
                    "Cache hit events by tier (l1|l2) and namespace — use rate() for ratio",
                    &["tier", "namespace"],
                    r
                )
                .unwrap(),
            )
            .ok();

        CDN_CACHE_STATUS_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_cdn_cache_status_total",
                    "CDN/edge cache response status by status code and path prefix",
                    &["status", "path_prefix"],
                    r
                )
                .unwrap(),
            )
            .ok();

        REDIS_MEMORY_USED_BYTES
            .set(
                register_gauge_vec_with_registry!(
                    "aframp_redis_memory_used_bytes",
                    "Redis used_memory in bytes (from INFO memory)",
                    &["instance"],
                    r
                )
                .unwrap(),
            )
            .ok();

        REDIS_MAXMEMORY_BYTES
            .set(
                register_gauge_vec_with_registry!(
                    "aframp_redis_maxmemory_bytes",
                    "Redis maxmemory configured limit in bytes (0 = unlimited)",
                    &["instance"],
                    r
                )
                .unwrap(),
            )
            .ok();
    }
}

// ---------------------------------------------------------------------------
// Database metrics
// ---------------------------------------------------------------------------

pub mod database {
    use super::*;

    static DB_QUERY_DURATION_SECONDS: OnceLock<HistogramVec> = OnceLock::new();
    static DB_CONNECTIONS_ACTIVE: OnceLock<GaugeVec> = OnceLock::new();
    static DB_ERRORS_TOTAL: OnceLock<CounterVec> = OnceLock::new();

    pub fn query_duration_seconds() -> &'static HistogramVec {
        DB_QUERY_DURATION_SECONDS
            .get()
            .expect("metrics not initialised")
    }

    pub fn connections_active() -> &'static GaugeVec {
        DB_CONNECTIONS_ACTIVE
            .get()
            .expect("metrics not initialised")
    }

    pub fn errors_total() -> &'static CounterVec {
        DB_ERRORS_TOTAL.get().expect("metrics not initialised")
    }

    pub(super) fn register(r: &Registry) {
        DB_QUERY_DURATION_SECONDS
            .set(
                register_histogram_vec_with_registry!(
                    "aframp_db_query_duration_seconds",
                    "Database query duration in seconds",
                    &["query_type", "table"],
                    vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0],
                    r
                )
                .unwrap(),
            )
            .ok();

        DB_CONNECTIONS_ACTIVE
            .set(
                register_gauge_vec_with_registry!(
                    "aframp_db_connections_active",
                    "Active database connections in the pool",
                    &["pool"],
                    r
                )
                .unwrap(),
            )
            .ok();

        DB_ERRORS_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_db_errors_total",
                    "Total database errors by error type",
                    &["error_type"],
                    r
                )
                .unwrap(),
            )
            .ok();
    }
}

// ---------------------------------------------------------------------------
// Security metrics
// Security / replay-prevention metrics  (Issue #141)
// ---------------------------------------------------------------------------

pub mod security {
    use super::*;

    static REQUEST_ANOMALY_FLAGS_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static REPLAY_ATTEMPTS_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static TIMESTAMP_REJECTIONS_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static TIMESTAMP_DELTA_SECONDS: OnceLock<HistogramVec> = OnceLock::new();

    pub fn request_anomaly_flags_total() -> &'static CounterVec {
        REQUEST_ANOMALY_FLAGS_TOTAL
            .get()
            .expect("metrics not initialised")
    }

    /// Increment when a replay is detected (nonce already seen).
    pub fn replay_attempts_total() -> &'static CounterVec {
        REPLAY_ATTEMPTS_TOTAL
            .get()
            .expect("metrics not initialised")
    }

    /// Increment when a request is rejected due to clock skew.
    pub fn timestamp_rejections_total() -> &'static CounterVec {
        TIMESTAMP_REJECTIONS_TOTAL
            .get()
            .expect("metrics not initialised")
    }

    /// Histogram of |server_time − request_timestamp| for valid requests.
    pub fn timestamp_delta_seconds() -> &'static HistogramVec {
        TIMESTAMP_DELTA_SECONDS
            .get()
            .expect("metrics not initialised")
    }

    pub(super) fn register(r: &Registry) {
        REQUEST_ANOMALY_FLAGS_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_request_anomaly_flags_total",
                    "Total non-blocking request anomaly flags by consumer, endpoint, and field",
                    &["consumer_id", "endpoint", "field"],
                    r
                )
                .unwrap(),
            )
            .ok();

        REPLAY_ATTEMPTS_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_security_replay_attempts_total",
                    "Total replay attempts detected, labelled by consumer_id and endpoint",
                    &["consumer_id", "endpoint"],
                    r
                )
                .unwrap(),
            )
            .ok();

        TIMESTAMP_REJECTIONS_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_security_timestamp_rejections_total",
                    "Total requests rejected due to timestamp clock skew",
                    &["consumer_id", "reason"],
                    r
                )
                .unwrap(),
            )
            .ok();

        TIMESTAMP_DELTA_SECONDS
            .set(
                register_histogram_vec_with_registry!(
                    "aframp_security_timestamp_delta_seconds",
                    "Distribution of |server_time - request_timestamp| for accepted requests",
                    &["consumer_id"],
                    vec![0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0],
                    r
                )
                .unwrap(),
            )
            .ok();
    }
}

// ---------------------------------------------------------------------------
// IP Detection & Blocking metrics (Issue #166)
// ---------------------------------------------------------------------------

pub mod ip_detection {
    use super::*;

    static IP_FLAGGED_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static IP_BLOCKS_APPLIED_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static IP_SHADOW_BLOCKS_APPLIED_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static IP_BLOCK_ENFORCEMENT_TOTAL: OnceLock<CounterVec> = OnceLock::new();
    static IP_AUTOMATED_BLOCKING_RATE: OnceLock<GaugeVec> = OnceLock::new();

    pub fn ip_flagged_total() -> &'static CounterVec {
        IP_FLAGGED_TOTAL.get().expect("metrics not initialised")
    }

    pub fn ip_blocks_applied_total() -> &'static CounterVec {
        IP_BLOCKS_APPLIED_TOTAL
            .get()
            .expect("metrics not initialised")
    }

    pub fn ip_shadow_blocks_applied_total() -> &'static CounterVec {
        IP_SHADOW_BLOCKS_APPLIED_TOTAL
            .get()
            .expect("metrics not initialised")
    }

    pub fn ip_block_enforcement_total() -> &'static CounterVec {
        IP_BLOCK_ENFORCEMENT_TOTAL
            .get()
            .expect("metrics not initialised")
    }

    pub fn ip_automated_blocking_rate() -> &'static GaugeVec {
        IP_AUTOMATED_BLOCKING_RATE
            .get()
            .expect("metrics not initialised")
    }

    pub(super) fn register(r: &Registry) {
        IP_FLAGGED_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_ip_flagged_total",
                    "Total IPs flagged by detection source",
                    &["detection_source", "evidence_type"],
                    r
                )
                .unwrap(),
            )
            .ok();

        IP_BLOCKS_APPLIED_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_ip_blocks_applied_total",
                    "Total IP blocks applied by block type",
                    &["block_type"],
                    r
                )
                .unwrap(),
            )
            .ok();

        IP_SHADOW_BLOCKS_APPLIED_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_ip_shadow_blocks_applied_total",
                    "Total IP shadow blocks applied",
                    &[],
                    r
                )
                .unwrap(),
            )
            .ok();

        IP_BLOCK_ENFORCEMENT_TOTAL
            .set(
                register_counter_vec_with_registry!(
                    "aframp_ip_block_enforcement_total",
                    "Total IP block enforcement events by endpoint and block type",
                    &["endpoint", "block_type"],
                    r
                )
                .unwrap(),
            )
            .ok();

        IP_AUTOMATED_BLOCKING_RATE
            .set(
                register_gauge_vec_with_registry!(
                    "aframp_ip_automated_blocking_rate",
                    "Rate of automated IP blocking per minute in the last 5 minutes",
                    &[],
                    r
                )
                .unwrap(),
            )
            .ok();
    }
}

// ---------------------------------------------------------------------------
// Register all metrics
// ---------------------------------------------------------------------------

fn register_all(r: &Registry) {
    http::register(r);
    cngn::register(r);
    payment::register(r);
    stellar::register(r);
    worker::register(r);
    cache::register(r);
    database::register(r);
    security::register(r);
    service_auth::register(r);
    ip_detection::register(r);
    alerting::register(r);
    issuer::register(r);
    crate::crypto::metrics::register(r);
    crate::admin::mint_signer_metrics::register(r);
    crate::key_management::metrics::register(r);
    crate::masking::metrics::register(r);
    crate::gateway::metrics::register(r);

    backup::register(r);
    por::register(r);
    #[cfg(feature = "database")]
    crate::analytics::metrics::register(r);
}

// ---------------------------------------------------------------------------
// Helper: extract key prefix from a Redis key (first colon-delimited segment)
// ---------------------------------------------------------------------------

pub fn key_prefix(key: &str) -> &str {
    key.find(':').map(|i| &key[..i]).unwrap_or(key)
}

// ---------------------------------------------------------------------------
// Backup metrics (Issue #119)
// ---------------------------------------------------------------------------

pub mod backup {
    use super::*;

    static LAST_SNAPSHOT_TIMESTAMP: OnceLock<GaugeVec> = OnceLock::new();
    static WAL_ARCHIVING_LAG: OnceLock<GaugeVec> = OnceLock::new();
    static VERIFICATION_STATUS: OnceLock<GaugeVec> = OnceLock::new();

    /// Record the Unix timestamp of the last successful snapshot.
    pub fn set_last_snapshot_timestamp(ts: f64) {
        if let Some(g) = LAST_SNAPSHOT_TIMESTAMP.get() {
            g.with_label_values(&[]).set(ts);
        }
    }

    /// Record the current WAL archiving lag in seconds.
    pub fn set_wal_lag(seconds: f64) {
        if let Some(g) = WAL_ARCHIVING_LAG.get() {
            g.with_label_values(&[]).set(seconds);
        }
    }

    /// Record the verification status of the latest snapshot (1 = verified, 0 = failed).
    pub fn set_verification_status(ok: bool) {
        if let Some(g) = VERIFICATION_STATUS.get() {
            g.with_label_values(&[]).set(if ok { 1.0 } else { 0.0 });
        }
    }

    pub(super) fn register(r: &Registry) {
        LAST_SNAPSHOT_TIMESTAMP
            .set(
                register_gauge_vec_with_registry!(
                    "aframp_backup_last_successful_snapshot_timestamp_seconds",
                    "Unix timestamp of the last successful database snapshot",
                    &[],
                    r
                )
                .unwrap(),
            )
            .ok();

        WAL_ARCHIVING_LAG
            .set(
                register_gauge_vec_with_registry!(
                    "aframp_backup_wal_archiving_lag_seconds",
                    "Seconds since the last WAL segment was archived",
                    &[],
                    r
                )
                .unwrap(),
            )
            .ok();

        VERIFICATION_STATUS
            .set(
                register_gauge_vec_with_registry!(
                    "aframp_backup_verification_status",
                    "Latest backup verification result: 1 = verified, 0 = failed",
                    &[],
                    r
                )
                .unwrap(),
            )
            .ok();
    }
}
