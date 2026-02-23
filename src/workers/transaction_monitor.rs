use crate::chains::stellar::client::{HorizonTransactionRecord, StellarClient};
use crate::database::repository::Repository;
use crate::database::transaction_repository::TransactionRepository;
use crate::database::webhook_repository::WebhookRepository;
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{error, info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Custom error type
// ---------------------------------------------------------------------------

/// Typed errors produced by the transaction monitor worker.
///
/// These are kept internal to the worker; callers at the `run` level receive
/// them only through logging — the worker loop never propagates a hard failure
/// upward so that a single bad transaction cannot crash the whole cycle.
#[derive(Debug, thiserror::Error)]
pub enum MonitorError {
    /// A transaction was still pending after the absolute deadline elapsed.
    #[error("confirmation timeout for transaction {tx_id} after {elapsed_secs}s")]
    ConfirmationTimeout { tx_id: String, elapsed_secs: u64 },

    /// A transaction exhausted all retry attempts without confirming.
    #[error("retry limit exceeded for transaction {tx_id} after {attempts} attempt(s)")]
    RetryExceeded { tx_id: String, attempts: u32 },

    /// A database operation failed.
    #[error("database error: {0}")]
    Database(#[from] crate::database::error::DatabaseError),

    /// A Stellar / Horizon network call failed.
    #[error("stellar error: {0}")]
    Stellar(String),

    /// Any other internal failure.
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<anyhow::Error> for MonitorError {
    fn from(e: anyhow::Error) -> Self {
        MonitorError::Internal(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TransactionMonitorConfig {
    /// How often the worker wakes up to poll Stellar.
    pub poll_interval: Duration,
    /// Absolute wall-clock deadline from `created_at`; transactions older than
    /// this are abandoned and marked `failed` (regardless of retry count).
    pub pending_timeout: Duration,
    /// Maximum number of retry attempts before a transaction is permanently
    /// failed (used in conjunction with the absolute timeout above).
    pub max_retries: u32,
    /// Maximum number of pending/processing transactions fetched per cycle.
    pub pending_batch_size: i64,
    /// How far back (in hours) to search for pending transactions.
    pub monitoring_window_hours: i32,
    /// Maximum number of incoming transactions fetched per cursor page.
    pub incoming_limit: usize,
    /// If set, the worker also scans this address for incoming cNGN payments.
    pub system_wallet_address: Option<String>,
}

impl Default for TransactionMonitorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(7),
            pending_timeout: Duration::from_secs(600),
            max_retries: 5,
            pending_batch_size: 200,
            monitoring_window_hours: 24,
            incoming_limit: 100,
            system_wallet_address: None,
        }
    }
}

impl TransactionMonitorConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        cfg.poll_interval = Duration::from_secs(
            std::env::var("TX_MONITOR_POLL_INTERVAL_SECONDS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(cfg.poll_interval.as_secs()),
        );
        cfg.pending_timeout = Duration::from_secs(
            std::env::var("TX_MONITOR_PENDING_TIMEOUT_SECONDS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(cfg.pending_timeout.as_secs()),
        );
        cfg.max_retries = std::env::var("TX_MONITOR_MAX_RETRIES")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(cfg.max_retries);
        cfg.pending_batch_size = std::env::var("TX_MONITOR_PENDING_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(cfg.pending_batch_size);
        cfg.monitoring_window_hours = std::env::var("TX_MONITOR_WINDOW_HOURS")
            .ok()
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(cfg.monitoring_window_hours);
        cfg.incoming_limit = std::env::var("TX_MONITOR_INCOMING_LIMIT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(cfg.incoming_limit);
        cfg.system_wallet_address = std::env::var("SYSTEM_WALLET_ADDRESS").ok();
        cfg
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

pub struct TransactionMonitorWorker {
    pool: PgPool,
    stellar_client: StellarClient,
    config: TransactionMonitorConfig,
    incoming_cursor: Option<String>,
}

impl TransactionMonitorWorker {
    pub fn new(
        pool: PgPool,
        stellar_client: StellarClient,
        config: TransactionMonitorConfig,
    ) -> Self {
        Self {
            pool,
            stellar_client,
            config,
            incoming_cursor: None,
        }
    }

    pub async fn run(mut self, mut shutdown_rx: watch::Receiver<bool>) {
        info!(
            poll_interval_secs = self.config.poll_interval.as_secs(),
            pending_timeout_secs = self.config.pending_timeout.as_secs(),
            max_retries = self.config.max_retries,
            monitoring_window_hours = self.config.monitoring_window_hours,
            has_system_wallet = self.config.system_wallet_address.is_some(),
            "stellar transaction monitor worker started"
        );

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("stellar transaction monitor worker stopping");
                        break;
                    }
                }
                _ = tokio::time::sleep(self.config.poll_interval) => {
                    if let Err(e) = self.run_cycle().await {
                        warn!(error = %e, "transaction monitor cycle failed");
                    }
                }
            }
        }

        info!("stellar transaction monitor worker stopped");
    }

    async fn run_cycle(&mut self) -> anyhow::Result<()> {
        self.process_pending_transactions().await?;
        self.scan_incoming_transactions().await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Pending-transaction polling
    // -----------------------------------------------------------------------

    async fn process_pending_transactions(&self) -> anyhow::Result<()> {
        let tx_repo = TransactionRepository::new(self.pool.clone());
        let pending = tx_repo
            .find_pending_payments_for_monitoring(
                self.config.monitoring_window_hours,
                self.config.pending_batch_size,
            )
            .await?;

        for tx in pending {
            let tx_id = tx.transaction_id.to_string();

            // ------------------------------------------------------------------
            // 1. Absolute timeout check (uses created_at so retries don't reset
            //    the clock).
            // ------------------------------------------------------------------
            if is_timed_out(tx.created_at, self.config.pending_timeout) {
                let elapsed = chrono::Utc::now()
                    .signed_duration_since(tx.created_at)
                    .to_std()
                    .unwrap_or_default();
                let err = MonitorError::ConfirmationTimeout {
                    tx_id: tx_id.clone(),
                    elapsed_secs: elapsed.as_secs(),
                };
                warn!(error = %err, "transaction exceeded absolute deadline");
                self.handle_absolute_timeout(&tx_id, Some(tx.metadata.clone()))
                    .await?;
                continue;
            }

            // ------------------------------------------------------------------
            // 2. Exponential backoff: skip transactions that have been retried
            //    but haven't waited long enough since the last attempt.
            // ------------------------------------------------------------------
            let retry_count = get_retry_count(Some(&tx.metadata));
            if retry_count > 0 && !is_ready_for_retry(tx.metadata.get("last_retry_at"), retry_count)
            {
                continue; // not yet ready; will be picked up in a later cycle
            }

            // ------------------------------------------------------------------
            // 3. We need a Stellar tx hash to poll Horizon.
            // ------------------------------------------------------------------
            let tx_hash = match extract_tx_hash(Some(&tx.metadata)) {
                Some(h) => h,
                None => {
                    warn!(
                        transaction_id = %tx_id,
                        "pending transaction has no stellar hash in metadata"
                    );
                    continue;
                }
            };

            // ------------------------------------------------------------------
            // 4. Query Horizon.
            // ------------------------------------------------------------------
            match self.stellar_client.get_transaction_by_hash(&tx_hash).await {
                Ok(record) => {
                    self.handle_horizon_status(
                        &tx_id,
                        record,
                        Some(tx.metadata.clone()),
                        retry_count,
                    )
                    .await?;
                }
                Err(e) => {
                    let message = e.to_string().to_lowercase();
                    // Transient states: keep polling without counting a retry.
                    if message.contains("not found")
                        || message.contains("network")
                        || message.contains("timeout")
                        || message.contains("rate limit")
                    {
                        continue;
                    }
                    self.fail_or_retry(&tx_id, Some(tx.metadata.clone()), &e.to_string())
                        .await?;
                }
            }
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Status handlers
    // -----------------------------------------------------------------------

    async fn handle_horizon_status(
        &self,
        transaction_id: &str,
        record: HorizonTransactionRecord,
        metadata: Option<JsonValue>,
        attempts: u32,
    ) -> anyhow::Result<()> {
        let mut updated = metadata.unwrap_or_else(|| json!({}));
        merge_status_fields(&mut updated, &record);

        let tx_repo = TransactionRepository::new(self.pool.clone());

        if record.successful {
            tx_repo
                .update_status_with_metadata(transaction_id, "completed", updated.clone())
                .await?;

            // Also write the confirmed hash to the dedicated column.
            tx_repo
                .update_blockchain_hash(transaction_id, &record.hash)
                .await?;

            info!(
                transaction_id = %transaction_id,
                tx_hash = %record.hash,
                ledger = ?record.ledger,
                attempts = attempts + 1,
                "transaction confirmed on stellar ledger"
            );

            self.log_webhook_event(transaction_id, "stellar.transaction.confirmed", updated)
                .await;
        } else {
            let reason = record
                .result_xdr
                .as_deref()
                .unwrap_or("transaction failed on horizon");
            self.fail_or_retry(transaction_id, Some(updated), reason)
                .await?;
        }
        Ok(())
    }

    /// Called when the absolute `pending_timeout` (measured from `created_at`)
    /// is exceeded. The transaction is failed immediately without further retries.
    async fn handle_absolute_timeout(
        &self,
        transaction_id: &str,
        metadata: Option<JsonValue>,
    ) -> anyhow::Result<()> {
        let mut updated = metadata.unwrap_or_else(|| json!({}));
        updated["last_monitor_error"] = json!("absolute pending timeout exceeded");
        updated["timed_out_at"] = json!(chrono::Utc::now().to_rfc3339());

        let tx_repo = TransactionRepository::new(self.pool.clone());
        tx_repo
            .update_status_with_metadata(transaction_id, "failed", updated.clone())
            .await?;

        self.log_webhook_event(transaction_id, "stellar.transaction.timeout", updated)
            .await;

        warn!(
            transaction_id = %transaction_id,
            "transaction failed: absolute timeout exceeded"
        );
        Ok(())
    }

    async fn fail_or_retry(
        &self,
        transaction_id: &str,
        metadata: Option<JsonValue>,
        error_message: &str,
    ) -> anyhow::Result<()> {
        let retries = next_retry_count(metadata.as_ref());
        let retryable = is_retryable_error(error_message);
        let mut updated = metadata.unwrap_or_else(|| json!({}));
        updated["last_monitor_error"] = json!(error_message);
        updated["retry_count"] = json!(retries);
        updated["last_retry_at"] = json!(chrono::Utc::now().to_rfc3339());
        updated["retryable"] = json!(retryable);

        let tx_repo = TransactionRepository::new(self.pool.clone());

        if retryable && retries <= self.config.max_retries {
            tx_repo
                .update_status_with_metadata(transaction_id, "pending", updated.clone())
                .await?;

            let next_backoff = backoff_delay(retries);
            warn!(
                transaction_id = %transaction_id,
                retry_count = retries,
                next_retry_after_secs = next_backoff.as_secs(),
                error = %error_message,
                "transaction failed with retryable error; scheduled for retry"
            );
        } else {
            tx_repo
                .update_status_with_metadata(transaction_id, "failed", updated.clone())
                .await?;

            let err = MonitorError::RetryExceeded {
                tx_id: transaction_id.to_string(),
                attempts: retries,
            };
            warn!(error = %err, original_error = %error_message, "transaction permanently failed");

            self.log_webhook_event(transaction_id, "stellar.transaction.failed", updated)
                .await;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Incoming payment scanning
    // -----------------------------------------------------------------------

    async fn scan_incoming_transactions(&mut self) -> anyhow::Result<()> {
        let system_wallet = match self.config.system_wallet_address.as_deref() {
            Some(addr) => addr,
            None => return Ok(()),
        };

        let page = self
            .stellar_client
            .list_account_transactions(
                system_wallet,
                self.config.incoming_limit,
                self.incoming_cursor.as_deref(),
            )
            .await?;

        let mut newest_cursor = self.incoming_cursor.clone();
        for tx in page.records {
            if !tx.successful {
                continue;
            }

            if let Some(cursor) = tx.paging_token.clone() {
                newest_cursor = Some(cursor);
            }

            let memo = match tx.memo.as_deref() {
                Some(m) if !m.trim().is_empty() => m,
                _ => continue,
            };

            let looks_like_incoming = self
                .is_incoming_cngn_payment(&tx.hash, system_wallet)
                .await
                .unwrap_or(false);
            if !looks_like_incoming {
                continue;
            }

            let tx_repo = TransactionRepository::new(self.pool.clone());
            match tx_repo.find_by_id(memo).await {
                Ok(Some(db_tx)) if db_tx.status == "pending" || db_tx.status == "processing" => {
                    let mut metadata = db_tx.metadata.clone();
                    metadata["incoming_hash"] = json!(tx.hash);
                    metadata["incoming_ledger"] = json!(tx.ledger);
                    metadata["incoming_confirmed_at"] = json!(chrono::Utc::now().to_rfc3339());
                    tx_repo
                        .update_status_with_metadata(
                            &db_tx.transaction_id.to_string(),
                            "completed",
                            metadata.clone(),
                        )
                        .await?;
                    // Also persist the confirmed hash to the dedicated column.
                    tx_repo
                        .update_blockchain_hash(&db_tx.transaction_id.to_string(), &tx.hash)
                        .await?;
                    info!(
                        transaction_id = %db_tx.transaction_id,
                        incoming_hash = %tx.hash,
                        ledger = ?tx.ledger,
                        "incoming cNGN payment matched and confirmed"
                    );
                    self.log_webhook_event(
                        &db_tx.transaction_id.to_string(),
                        "stellar.incoming.matched",
                        metadata,
                    )
                    .await;
                }
                Ok(_) => {
                    self.log_unmatched_incoming(memo, &tx).await;
                }
                Err(e) => {
                    warn!(
                        memo = %memo,
                        error = %e,
                        "failed to look up memo for incoming transaction"
                    );
                }
            }
        }

        self.incoming_cursor = newest_cursor;
        Ok(())
    }

    async fn is_incoming_cngn_payment(
        &self,
        tx_hash: &str,
        system_wallet: &str,
    ) -> anyhow::Result<bool> {
        let issuer = std::env::var("CNGN_ISSUER_TESTNET")
            .or_else(|_| std::env::var("CNGN_ISSUER_MAINNET"))
            .unwrap_or_default();
        let operations = self
            .stellar_client
            .get_transaction_operations(tx_hash)
            .await?;
        for op in operations {
            let op_type = op.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if op_type != "payment" {
                continue;
            }

            let destination = op.get("to").and_then(|v| v.as_str()).unwrap_or("");
            let asset_code = op.get("asset_code").and_then(|v| v.as_str()).unwrap_or("");
            let asset_issuer = op
                .get("asset_issuer")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if destination == system_wallet && asset_code.eq_ignore_ascii_case("cngn") {
                if issuer.is_empty() || asset_issuer == issuer {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // -----------------------------------------------------------------------
    // Webhook helpers
    // -----------------------------------------------------------------------

    async fn log_webhook_event(&self, transaction_id: &str, event_type: &str, payload: JsonValue) {
        let parsed_tx_id = Uuid::parse_str(transaction_id).ok();
        let repo = WebhookRepository::new(self.pool.clone());
        let event_id = format!("{}:{}", event_type, transaction_id);
        if let Err(e) = repo
            .log_event(
                &event_id,
                "stellar",
                event_type,
                payload,
                None,
                parsed_tx_id,
            )
            .await
        {
            warn!(
                transaction_id = %transaction_id,
                event_type = %event_type,
                error = %e,
                "failed to write webhook event"
            );
        }
    }

    async fn log_unmatched_incoming(&self, memo: &str, tx: &HorizonTransactionRecord) {
        let repo = WebhookRepository::new(self.pool.clone());
        let event_id = format!("unmatched:{}", tx.hash);
        let payload = json!({
            "memo": memo,
            "hash": tx.hash,
            "ledger": tx.ledger,
            "created_at": tx.created_at,
        });
        if let Err(e) = repo
            .log_event(
                &event_id,
                "stellar",
                "stellar.incoming.unmatched",
                payload,
                None,
                None,
            )
            .await
        {
            error!(error = %e, "failed to log unmatched incoming transaction");
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helper functions
// ---------------------------------------------------------------------------

/// Extract the Stellar transaction hash from metadata, trying several known keys.
fn extract_tx_hash(metadata: Option<&JsonValue>) -> Option<String> {
    let metadata = metadata?;
    for key in [
        "submitted_hash",
        "stellar_tx_hash",
        "transaction_hash",
        "hash",
    ] {
        if let Some(value) = metadata.get(key).and_then(|v| v.as_str()) {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Return the current retry count stored in metadata (0 if absent).
fn get_retry_count(metadata: Option<&JsonValue>) -> u32 {
    metadata
        .and_then(|m| m.get("retry_count"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32
}

/// Increment the retry count by 1 (used when scheduling the next retry).
fn next_retry_count(metadata: Option<&JsonValue>) -> u32 {
    get_retry_count(metadata) + 1
}

/// Exponential backoff schedule for retry attempts.
///
/// | attempt | delay   |
/// |---------|---------|
/// | 0       | 0 s     |
/// | 1       | 10 s    |
/// | 2       | 30 s    |
/// | 3       | 2 min   |
/// | 4       | 5 min   |
/// | ≥ 5     | 10 min  |
pub fn backoff_delay(retry_count: u32) -> Duration {
    match retry_count {
        0 => Duration::from_secs(0),
        1 => Duration::from_secs(10),
        2 => Duration::from_secs(30),
        3 => Duration::from_secs(120),
        4 => Duration::from_secs(300),
        _ => Duration::from_secs(600),
    }
}

/// Returns `true` if enough time has elapsed since `last_retry_at` to allow
/// the next polling attempt, according to the exponential backoff schedule.
fn is_ready_for_retry(last_retry_at: Option<&JsonValue>, retry_count: u32) -> bool {
    let delay = backoff_delay(retry_count);
    if delay.is_zero() {
        return true;
    }

    let last = last_retry_at
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));

    match last {
        None => true, // no prior attempt recorded → eligible immediately
        Some(last_dt) => {
            let elapsed = chrono::Utc::now() - last_dt;
            elapsed.to_std().map(|d| d >= delay).unwrap_or(true)
        }
    }
}

/// Returns `true` when `last_updated` is older than `timeout` (used for the
/// absolute deadline check against `created_at`).
fn is_timed_out(last_updated: chrono::DateTime<chrono::Utc>, timeout: Duration) -> bool {
    let elapsed = chrono::Utc::now() - last_updated;
    elapsed.to_std().map(|d| d > timeout).unwrap_or(false)
}

/// Returns `true` for transient Stellar error codes that are worth retrying.
fn is_retryable_error(message: &str) -> bool {
    let m = message.to_lowercase();
    m.contains("tx_bad_seq")
        || m.contains("bad sequence")
        || m.contains("tx_insufficient_fee")
        || m.contains("insufficient fee")
        || m.contains("timeout")
        || m.contains("rate limit")
        || m.contains("network")
}

/// Merge Horizon response fields into the metadata object.
fn merge_status_fields(metadata: &mut JsonValue, record: &HorizonTransactionRecord) {
    metadata["submitted_hash"] = json!(record.hash);
    metadata["confirmed_ledger"] = json!(record.ledger);
    metadata["confirmed_at"] = json!(record.created_at);
    metadata["horizon_successful"] = json!(record.successful);
    if let Some(result_xdr) = &record.result_xdr {
        metadata["result_xdr"] = json!(result_xdr);
    }
    if let Some(result_meta_xdr) = &record.result_meta_xdr {
        metadata["result_meta_xdr"] = json!(result_meta_xdr);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> HorizonTransactionRecord {
        HorizonTransactionRecord {
            id: Some("id_1".to_string()),
            paging_token: Some("123".to_string()),
            hash: "stellar_hash_1".to_string(),
            successful: true,
            ledger: Some(9876),
            created_at: Some("2026-02-12T00:00:00Z".to_string()),
            memo_type: Some("text".to_string()),
            memo: Some("memo-1".to_string()),
            result_xdr: Some("result_xdr_1".to_string()),
            result_meta_xdr: Some("result_meta_xdr_1".to_string()),
            envelope_xdr: Some("envelope_xdr_1".to_string()),
            fee_charged: Some("100".to_string()),
        }
    }

    // --- is_retryable_error -------------------------------------------------

    #[test]
    fn retryable_error_codes_are_detected() {
        assert!(is_retryable_error("tx_bad_seq"));
        assert!(is_retryable_error("tx_insufficient_fee"));
        assert!(is_retryable_error("network error while checking horizon"));
        assert!(is_retryable_error("rate limit exceeded"));
        assert!(!is_retryable_error("op_underfunded"));
    }

    // --- extract_tx_hash ----------------------------------------------------

    #[test]
    fn hash_extraction_uses_known_keys() {
        let meta = json!({"transaction_hash": "abc"});
        assert_eq!(extract_tx_hash(Some(&meta)).as_deref(), Some("abc"));
    }

    #[test]
    fn hash_extraction_prefers_submitted_hash_first() {
        let meta = json!({
            "submitted_hash": "preferred",
            "stellar_tx_hash": "secondary",
            "transaction_hash": "tertiary"
        });
        assert_eq!(extract_tx_hash(Some(&meta)).as_deref(), Some("preferred"));
    }

    #[test]
    fn hash_extraction_ignores_empty_values() {
        let meta = json!({
            "submitted_hash": "",
            "stellar_tx_hash": "",
            "transaction_hash": "fallback"
        });
        assert_eq!(extract_tx_hash(Some(&meta)).as_deref(), Some("fallback"));
    }

    // --- retry counts -------------------------------------------------------

    #[test]
    fn retry_count_defaults_to_zero() {
        assert_eq!(get_retry_count(None), 0);
        let meta = json!({});
        assert_eq!(get_retry_count(Some(&meta)), 0);
    }

    #[test]
    fn retry_count_reads_stored_value() {
        let meta = json!({ "retry_count": 4 });
        assert_eq!(get_retry_count(Some(&meta)), 4);
    }

    #[test]
    fn next_retry_count_increments() {
        assert_eq!(next_retry_count(None), 1);
        let meta = json!({ "retry_count": 4 });
        assert_eq!(next_retry_count(Some(&meta)), 5);
    }

    // --- timeout detection --------------------------------------------------

    #[test]
    fn timeout_detection_is_correct() {
        let now = chrono::Utc::now();
        let very_recent = now - chrono::Duration::seconds(5);
        let old = now - chrono::Duration::seconds(120);

        assert!(!is_timed_out(very_recent, Duration::from_secs(30)));
        assert!(is_timed_out(old, Duration::from_secs(30)));
    }

    // --- exponential backoff schedule ---------------------------------------

    #[test]
    fn backoff_delay_schedule_is_correct() {
        assert_eq!(backoff_delay(0), Duration::from_secs(0));
        assert_eq!(backoff_delay(1), Duration::from_secs(10));
        assert_eq!(backoff_delay(2), Duration::from_secs(30));
        assert_eq!(backoff_delay(3), Duration::from_secs(120));
        assert_eq!(backoff_delay(4), Duration::from_secs(300));
        assert_eq!(backoff_delay(5), Duration::from_secs(600));
        assert_eq!(backoff_delay(99), Duration::from_secs(600)); // capped
    }

    // --- is_ready_for_retry -------------------------------------------------

    #[test]
    fn first_attempt_always_ready() {
        // retry_count == 0 → delay == 0 → always ready
        assert!(is_ready_for_retry(None, 0));
        assert!(is_ready_for_retry(Some(&json!("2026-01-01T00:00:00Z")), 0));
    }

    #[test]
    fn missing_last_retry_at_is_ready() {
        // If metadata has a non-zero retry_count but no timestamp, treat as ready.
        assert!(is_ready_for_retry(None, 3));
    }

    #[test]
    fn recent_last_retry_at_is_not_ready() {
        // retry 1 → 10 s delay; timestamp 2 s ago → not ready
        let two_seconds_ago = (chrono::Utc::now() - chrono::Duration::seconds(2)).to_rfc3339();
        let ts = json!(two_seconds_ago);
        assert!(!is_ready_for_retry(Some(&ts), 1));
    }

    #[test]
    fn old_enough_last_retry_at_is_ready() {
        // retry 1 → 10 s delay; timestamp 15 s ago → ready
        let fifteen_seconds_ago = (chrono::Utc::now() - chrono::Duration::seconds(15)).to_rfc3339();
        let ts = json!(fifteen_seconds_ago);
        assert!(is_ready_for_retry(Some(&ts), 1));
    }

    #[test]
    fn retry_2_requires_30s_gap() {
        let twenty_seconds_ago = (chrono::Utc::now() - chrono::Duration::seconds(20)).to_rfc3339();
        let ts = json!(twenty_seconds_ago);
        assert!(!is_ready_for_retry(Some(&ts), 2), "20s < 30s → not ready");

        let thirty_five_seconds_ago =
            (chrono::Utc::now() - chrono::Duration::seconds(35)).to_rfc3339();
        let ts2 = json!(thirty_five_seconds_ago);
        assert!(is_ready_for_retry(Some(&ts2), 2), "35s > 30s → ready");
    }

    // --- merge_status_fields ------------------------------------------------

    #[test]
    fn merge_status_fields_copies_core_tracking_values() {
        let mut metadata = json!({});
        let record = sample_record();

        merge_status_fields(&mut metadata, &record);

        assert_eq!(metadata["submitted_hash"], json!("stellar_hash_1"));
        assert_eq!(metadata["confirmed_ledger"], json!(9876));
        assert_eq!(metadata["confirmed_at"], json!("2026-02-12T00:00:00Z"));
        assert_eq!(metadata["horizon_successful"], json!(true));
        assert_eq!(metadata["result_xdr"], json!("result_xdr_1"));
        assert_eq!(metadata["result_meta_xdr"], json!("result_meta_xdr_1"));
    }

    #[test]
    fn merge_status_fields_skips_optional_xdr_when_missing() {
        let mut metadata = json!({});
        let mut record = sample_record();
        record.result_xdr = None;
        record.result_meta_xdr = None;

        merge_status_fields(&mut metadata, &record);

        assert!(metadata.get("result_xdr").is_none());
        assert!(metadata.get("result_meta_xdr").is_none());
        assert_eq!(metadata["submitted_hash"], json!("stellar_hash_1"));
    }

    // --- MonitorError display -----------------------------------------------

    #[test]
    fn monitor_error_timeout_message() {
        let e = MonitorError::ConfirmationTimeout {
            tx_id: "abc-123".to_string(),
            elapsed_secs: 610,
        };
        assert!(e.to_string().contains("abc-123"));
        assert!(e.to_string().contains("610"));
    }

    #[test]
    fn monitor_error_retry_exceeded_message() {
        let e = MonitorError::RetryExceeded {
            tx_id: "def-456".to_string(),
            attempts: 5,
        };
        assert!(e.to_string().contains("def-456"));
        assert!(e.to_string().contains('5'));
    }
}
