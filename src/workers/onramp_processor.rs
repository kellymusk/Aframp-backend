//! Onramp Transaction Processor Worker
//!
//! The engine room of the entire onramp flow. This background worker:
//! - Polls pending transactions for payment provider confirmation (fallback for missed webhooks)
//! - Processes webhook-triggered payment confirmations
//! - Executes AFRI/cNGN transfers on Stellar once fiat payment is confirmed
//! - Monitors Stellar for on-chain confirmation via the TransactionMonitorWorker
//! - Handles all failure scenarios including automatic fiat refunds
//!
//! State machine: pending → payment_received → processing → completed
//!                                                        ↘ refund_initiated → refunded
//!
//! This is the highest-stakes code in the codebase — it moves real money.
//! Every state transition is idempotent and race-condition proof via
//! optimistic locking (WHERE status = '<expected>').

use crate::chains::stellar::client::StellarClient;
use crate::chains::stellar::errors::StellarError;
use crate::chains::stellar::payment::{CngnMemo, CngnPaymentBuilder};
use crate::chains::stellar::trustline::CngnTrustlineManager;
use crate::database::transaction_repository::{Transaction, TransactionRepository};
use crate::audit::mint_authorization::{MintAuthorizationService, MintActionType};
use crate::payments::factory::PaymentProviderFactory;
use crate::payments::types::{PaymentState, ProviderName, StatusRequest};
use bigdecimal::BigDecimal;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::{interval, sleep};
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for the Onramp Transaction Processor.
/// All values are tunable via environment variables.
#[derive(Debug, Clone)]
pub struct OnrampProcessorConfig {
    /// How often the polling fallback wakes up (seconds)
    pub poll_interval_secs: u64,
    /// Mark pending transactions failed after this duration (minutes)
    pub pending_timeout_mins: u64,
    /// Maximum retry attempts for Stellar submission
    pub stellar_max_retries: u32,
    /// Exponential backoff delays for Stellar retries (seconds)
    pub stellar_retry_backoff_secs: Vec<u64>,
    /// How often to poll Stellar for confirmation (seconds)
    pub stellar_confirmation_poll_secs: u64,
    /// Absolute timeout for Stellar confirmation (minutes)
    pub stellar_confirmation_timeout_mins: u64,
    /// Maximum retry attempts for fiat refunds
    pub refund_max_retries: u32,
    /// Exponential backoff delays for refund retries (seconds)
    pub refund_retry_backoff_secs: Vec<u64>,
    /// System wallet Stellar address (source of cNGN transfers)
    pub system_wallet_address: String,
    /// System wallet secret seed (signing key for Stellar transactions)
    pub system_wallet_secret: String,
}

impl Default for OnrampProcessorConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 30,
            pending_timeout_mins: 30,
            stellar_max_retries: 3,
            stellar_retry_backoff_secs: vec![2, 4, 8],
            stellar_confirmation_poll_secs: 10,
            stellar_confirmation_timeout_mins: 5,
            refund_max_retries: 3,
            refund_retry_backoff_secs: vec![30, 60, 120],
            system_wallet_address: String::new(),
            system_wallet_secret: String::new(),
        }
    }
}

impl OnrampProcessorConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        cfg.poll_interval_secs = std::env::var("ONRAMP_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(cfg.poll_interval_secs);
        cfg.pending_timeout_mins = std::env::var("ONRAMP_PENDING_TIMEOUT_MINS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(cfg.pending_timeout_mins);
        cfg.stellar_max_retries = std::env::var("ONRAMP_STELLAR_MAX_RETRIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(cfg.stellar_max_retries);
        cfg.stellar_confirmation_timeout_mins =
            std::env::var("ONRAMP_STELLAR_CONFIRMATION_TIMEOUT_MINS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(cfg.stellar_confirmation_timeout_mins);
        cfg.system_wallet_address = std::env::var("SYSTEM_WALLET_ADDRESS").unwrap_or_default();
        cfg.system_wallet_secret = std::env::var("SYSTEM_WALLET_SECRET").unwrap_or_default();
        cfg
    }
}

// ============================================================================
// Error Types
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum ProcessorError {
    #[error("database error: {0}")]
    Database(#[from] crate::database::error::DatabaseError),

    #[error("stellar error: {0}")]
    Stellar(String),

    #[error("payment provider error: {0}")]
    PaymentProvider(String),

    #[error("trustline not found for wallet {wallet_address}")]
    TrustlineNotFound { wallet_address: String },

    #[error("insufficient cNGN balance: available={available}, required={required}")]
    InsufficientCngnBalance { available: String, required: String },

    #[error("payment timeout after {mins} minutes")]
    PaymentTimeout { mins: u64 },

    /// Transient Stellar error — safe to retry with backoff
    #[error("stellar transient error: {0}")]
    StellarTransientError(String),

    /// Permanent Stellar error — do not retry, trigger refund
    #[error("stellar permanent error: {0}")]
    StellarPermanentError(String),

    #[error("refund failed: {0}")]
    RefundFailed(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl ProcessorError {
    /// Returns true if this error is safe to retry with backoff.
    pub fn is_transient(&self) -> bool {
        matches!(self, ProcessorError::StellarTransientError(_))
    }
}

impl From<sqlx::Error> for ProcessorError {
    fn from(e: sqlx::Error) -> Self {
        ProcessorError::Internal(e.to_string())
    }
}

impl From<StellarError> for ProcessorError {
    fn from(e: StellarError) -> Self {
        match &e {
            // Transient — network hiccups, rate limits, timeouts
            StellarError::NetworkError { .. }
            | StellarError::TimeoutError { .. }
            | StellarError::RateLimitError => ProcessorError::StellarTransientError(e.to_string()),
            // Permanent — bad sequence, signing failure, serialization
            _ => ProcessorError::StellarPermanentError(e.to_string()),
        }
    }
}

// ============================================================================
// Failure Reasons (stored in DB for audit trail)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FailureReason {
    PaymentTimeout,
    PaymentFailed,
    TrustlineNotFound,
    InsufficientCngnBalance,
    StellarTransientError,
    StellarPermanentError,
    UnknownError,
}

impl FailureReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            FailureReason::PaymentTimeout => "PAYMENT_TIMEOUT",
            FailureReason::PaymentFailed => "PAYMENT_FAILED",
            FailureReason::TrustlineNotFound => "TRUSTLINE_NOT_FOUND",
            FailureReason::InsufficientCngnBalance => "INSUFFICIENT_CNGN_BALANCE",
            FailureReason::StellarTransientError => "STELLAR_TRANSIENT_ERROR",
            FailureReason::StellarPermanentError => "STELLAR_PERMANENT_ERROR",
            FailureReason::UnknownError => "UNKNOWN_ERROR",
        }
    }
}

// ============================================================================
// Processor Worker
// ============================================================================

pub struct OnrampProcessor {
    db: Arc<PgPool>,
    stellar: Arc<StellarClient>,
    provider_factory: Arc<PaymentProviderFactory>,
    config: OnrampProcessorConfig,
    mint_audit_service: Option<Arc<MintAuthorizationService>>,
}

impl OnrampProcessor {
    pub fn new(
        db: PgPool,
        stellar: StellarClient,
        provider_factory: Arc<PaymentProviderFactory>,
        config: OnrampProcessorConfig,
        mint_audit_service: Option<Arc<MintAuthorizationService>>,
    ) -> Self {
        Self {
            db: Arc::new(db),
            stellar: Arc::new(stellar),
            provider_factory,
            config,
            mint_audit_service,
        }
    }

    // =========================================================================
    // Main Loop
    // =========================================================================

    /// Main worker loop — runs continuously until shutdown signal.
    pub async fn run(&self, mut shutdown_rx: watch::Receiver<bool>) -> Result<(), ProcessorError> {
        if self.config.system_wallet_address.is_empty()
            || self.config.system_wallet_secret.is_empty()
        {
            return Err(ProcessorError::Config(
                "SYSTEM_WALLET_ADDRESS and SYSTEM_WALLET_SECRET must be set".to_string(),
            ));
        }

        info!(
            poll_interval_secs = self.config.poll_interval_secs,
            pending_timeout_mins = self.config.pending_timeout_mins,
            stellar_max_retries = self.config.stellar_max_retries,
            "🚀 Onramp Transaction Processor started"
        );

        let mut ticker = interval(Duration::from_secs(self.config.poll_interval_secs));

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    info!("Shutdown signal received, stopping Onramp Processor");
                    break;
                }
                _ = ticker.tick() => {
                    if let Err(e) = self.process_cycle().await {
                        error!(error = %e, "Error in onramp processor cycle");
                    }
                }
            }
        }

        info!("Onramp Transaction Processor stopped");
        Ok(())
    }

    /// Single processing cycle — three stages run sequentially.
    async fn process_cycle(&self) -> Result<(), ProcessorError> {
        debug!("Starting onramp processor cycle");
        self.expire_timed_out_payments().await?;
        self.poll_pending_transactions().await?;

        let _timer = crate::metrics::worker::cycle_duration_seconds()
            .with_label_values(&["onramp_processor"])
            .start_timer();
        crate::metrics::worker::cycles_total()
            .with_label_values(&["onramp_processor"])
            .inc();

        // Stage 1: Check for payment timeouts
        self.check_payment_timeouts().await?;

        // Stage 2: Process pending transactions (polling fallback)
        self.process_pending_transactions().await?;

        // Stage 3: Monitor Stellar confirmations
        self.monitor_stellar_confirmations().await?;

        // Update last-cycle timestamp for missed-cycle alert rule
        #[cfg(feature = "cache")]
        crate::metrics::alerting::worker_last_cycle_timestamp()
            .with_label_values(&["onramp_processor"])
            .set(chrono::Utc::now().timestamp() as f64);

        debug!("Onramp processor cycle complete");
        Ok(())
    }

    // =========================================================================
    // Stage 1 — Expire timed-out pending payments
    // =========================================================================

    /// Mark pending transactions as failed when they exceed the configured timeout.
    /// No refund is needed here — fiat was never confirmed.
    #[instrument(skip(self), fields(worker = "onramp"))]
    async fn expire_timed_out_payments(&self) -> Result<(), ProcessorError> {
        let timed_out: Vec<Transaction> = sqlx::query_as::<_, Transaction>(
            r#"
            SELECT transaction_id, wallet_address, type, from_currency, to_currency,
                   from_amount, to_amount, cngn_amount, status, payment_provider,
                   payment_reference, blockchain_tx_hash, error_message, metadata,
                   created_at, updated_at
            FROM transactions
            WHERE type = 'onramp'
              AND status = 'pending'
              AND created_at < NOW() - ($1 * INTERVAL '1 minute')
            ORDER BY created_at ASC
            LIMIT 100
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(self.config.pending_timeout_mins as i64)
        .fetch_all((*self.db).as_ref())
        .await?;

        for tx in timed_out {
            warn!(
                tx_id = %tx.transaction_id,
                age_mins = self.config.pending_timeout_mins,
                "Payment confirmation timeout — marking failed"
            );
            self.transition_failed(
                &tx.transaction_id,
                "pending",
                FailureReason::PaymentTimeout,
                &format!(
                    "No payment confirmation received within {} minutes",
                    self.config.pending_timeout_mins
                ),
            )
            .await?;

            // Emit metric
            metrics::inc_payments_failed("timeout");
        }

        Ok(())
    }

    // =========================================================================
    // Stage 2 — Poll provider for missed webhooks
    // =========================================================================

    /// Polling fallback: find pending onramp transactions that have no associated
    /// webhook event and query the payment provider directly.
    #[instrument(skip(self), fields(worker = "onramp"))]
    async fn poll_pending_transactions(&self) -> Result<(), ProcessorError> {
        // Only poll transactions older than 2 minutes with no webhook received
        let stale: Vec<Transaction> = sqlx::query_as::<_, Transaction>(
            r#"
            SELECT t.transaction_id, t.wallet_address, t.type, t.from_currency, t.to_currency,
                   t.from_amount, t.to_amount, t.cngn_amount, t.status, t.payment_provider,
                   t.payment_reference, t.blockchain_tx_hash, t.error_message, t.metadata,
                   t.created_at, t.updated_at
            FROM transactions t
            LEFT JOIN webhook_events w
                   ON t.transaction_id = w.transaction_id
                  AND w.status IN ('completed', 'processing')
            WHERE t.type = 'onramp'
              AND t.status = 'pending'
              AND t.created_at < NOW() - INTERVAL '2 minutes'
              AND w.id IS NULL
            ORDER BY t.created_at ASC
            LIMIT 50
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .fetch_all((*self.db).as_ref())
        .await?;

        for tx in stale {
            debug!(
                tx_id = %tx.transaction_id,
                provider = ?tx.payment_provider,
                "Polling provider for missed payment confirmation"
            );
            if let Err(e) = self.query_provider_status(&tx).await {
                warn!(
                    tx_id = %tx.transaction_id,
                    error = %e,
                    "Provider status poll failed"
                );
            }
        }

        Ok(())
    }

    /// Query the payment provider for the current status of a pending transaction.
    /// If confirmed, drive it through the full processing pipeline.
    async fn query_provider_status(&self, tx: &Transaction) -> Result<(), ProcessorError> {
        let provider_ref = match tx.payment_reference.as_deref() {
            Some(r) => r,
            None => {
                debug!(tx_id = %tx.transaction_id, "No payment reference, skipping poll");
                return Ok(());
            }
        };
        let provider_name = match tx.payment_provider.as_deref() {
            Some(p) => p,
            None => {
                debug!(tx_id = %tx.transaction_id, "No payment provider, skipping poll");
                return Ok(());
            }
        };

        let provider_enum: ProviderName = provider_name
            .parse()
            .map_err(|e: crate::payments::error::PaymentError| {
                ProcessorError::PaymentProvider(e.to_string())
            })?;

        let provider = self
            .provider_factory
            .get_provider(provider_enum.clone())
            .map_err(|e| ProcessorError::PaymentProvider(e.to_string()))?;

        let status_req = StatusRequest {
            transaction_reference: tx.payment_reference.clone(),
            provider_reference: None,
        };

        let status_resp = provider
            .verify_payment(status_req)
            .await
            .map_err(|e| ProcessorError::PaymentProvider(e.to_string()))?;

        match status_resp.status {
            PaymentState::Success => {
                info!(
                    tx_id = %tx.transaction_id,
                    provider = %provider_name,
                    "Provider poll: payment confirmed — driving to processing"
                );
                let amount = tx.from_amount.clone();
                self.process_payment_confirmed(
                    &tx.transaction_id,
                    provider_ref,
                    &provider_enum,
                    &amount,
                )
                .await?;
            }
            PaymentState::Failed | PaymentState::Cancelled | PaymentState::Reversed => {
                warn!(
                    tx_id = %tx.transaction_id,
                    provider = %provider_name,
                    status = ?status_resp.status,
                    "Provider poll: payment failed"
                );
                self.transition_failed(
                    &tx.transaction_id,
                    "pending",
                    FailureReason::PaymentFailed,
                    &format!("Payment provider reported status: {:?}", status_resp.status),
                )
                .await?;
            }
            _ => {
                debug!(
                    tx_id = %tx.transaction_id,
                    status = ?status_resp.status,
                    "Provider poll: payment still pending"
                );
            }
        }

        Ok(())
    }

    /// Check payment status with provider directly
    async fn check_payment_with_provider(&self, tx: &Transaction) -> Result<(), ProcessorError> {
        let provider_ref = tx
            .payment_reference
            .as_ref()
            .ok_or_else(|| ProcessorError::Internal("No payment reference".to_string()))?;

        let provider_name = tx
            .payment_provider
            .as_ref()
            .ok_or_else(|| ProcessorError::Internal("No payment provider".to_string()))?;

        // Query provider for status
        // This would call the payment orchestrator's status check
        // For now, we log the attempt
        debug!(
            tx_id = %tx.transaction_id,
            provider = %provider_name,
            provider_ref = %provider_ref,
            "Querying provider for payment status"
        );

        // TODO: Implement provider status query via PaymentOrchestrator
        // If confirmed, call process_payment_confirmed()

        Ok(())
    }

    /// Monitor Stellar for transaction confirmations
    #[instrument(skip(self), fields(processor = "onramp"))]
    async fn monitor_stellar_confirmations(&self) -> Result<(), ProcessorError> {
        // Find transactions awaiting Stellar confirmation
        let processing_txs = sqlx::query_as::<_, Transaction>(
            r#"
            SELECT * FROM transactions
            WHERE status = 'processing'
            AND blockchain_tx_hash IS NOT NULL
            ORDER BY updated_at ASC
            LIMIT 50
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .fetch_all((*self.db).as_ref())
        .await?;

        for tx in processing_txs {
            let tx_hash = tx
                .blockchain_tx_hash
                .as_ref()
                .ok_or_else(|| ProcessorError::Internal("No blockchain hash".to_string()))?;

            debug!(
                tx_id = %tx.transaction_id,
                stellar_hash = %tx_hash,
                "Checking Stellar transaction confirmation"
            );

            match self.check_stellar_confirmation(tx_hash).await {
                Ok(confirmed) => {
                    if confirmed {
                        info!(
                            tx_id = %tx.transaction_id,
                            stellar_hash = %tx_hash,
                            "Stellar transaction confirmed"
                        );

                        // Mark as completed
                        self.mark_transaction_completed(&tx.transaction_id, tx_hash).await?;
                        metrics::inc_transfers_confirmed();
                    }
                }
                Err(e) => {
                    warn!(
                        tx_id = %tx.transaction_id,
                        error = %e,
                        "Error checking Stellar confirmation"
                    );
                }
            }
        }

        Ok(())
    }

    // =========================================================================
    // Payment Confirmation Entry Point (webhook + polling)
    // =========================================================================

    /// Process a confirmed fiat payment. Called by both the webhook handler and
    /// the polling fallback. Fully idempotent — safe to call multiple times for
    /// the same payment event.
    ///
    /// State transition: pending → payment_received → processing
    #[instrument(
        skip(self, amount_ngn),
        fields(worker = "onramp", tx_id = %tx_id, provider = %provider)
    )]
    pub async fn process_payment_confirmed(
        &self,
        tx_id: &Uuid,
        provider_reference: &str,
        provider: &ProviderName,
        amount_ngn: &BigDecimal,
    ) -> Result<(), ProcessorError> {
        info!(
            tx_id = %tx_id,
            provider = %provider,
            provider_ref = %provider_reference,
            amount_ngn = %amount_ngn,
            "Fiat payment confirmed — beginning onramp processing"
        );

        // Fetch the transaction record
        let repo = TransactionRepository::new((*self.db).clone());
        let tx = repo
            .find_by_id(&tx_id.to_string())
            .await?
            .ok_or_else(|| ProcessorError::Internal(format!("Transaction not found: {}", tx_id)))?;

        // Guard: already past pending — idempotent no-op
        if tx.status != "pending" {
            info!(
                tx_id = %tx_id,
                current_status = %tx.status,
                "Transaction already past pending state — skipping (idempotent)"
            );
            return Ok(());
        }

        // Validate amount matches what was quoted
        if tx.from_amount != *amount_ngn {
            warn!(
                tx_id = %tx_id,
                expected = %tx.from_amount,
                received = %amount_ngn,
                "Payment amount mismatch — rejecting confirmation"
            );
            self.transition_failed(
                tx_id,
                "pending",
                FailureReason::PaymentFailed,
                &format!(
                    "Amount mismatch: expected {}, received {}",
                    tx.from_amount, amount_ngn
                ),
            )
            .await?;
            return Ok(());
        }

        // Transition: pending → payment_received (optimistic lock)
        let rows = sqlx::query(
            r#"
            UPDATE transactions
            SET status = 'payment_received',
                metadata = metadata || $1,
                updated_at = NOW()
            WHERE transaction_id = $2
              AND status = 'pending'
            "#,
        )
        .bind(json!({
            "payment_confirmed_at": Utc::now().to_rfc3339(),
            "provider_reference": provider_reference,
            "confirmed_amount_ngn": amount_ngn.to_string(),
        }))
        .bind(tx_id)
        .execute((*self.db).as_ref())
        .await?
        .rows_affected();

        if rows == 0 {
            // Another process already handled this — idempotent exit
            info!(
                tx_id = %tx_id,
                "Optimistic lock miss on pending→payment_received — already processed"
            );
            return Ok(());
        }

        info!(
            tx_id = %tx_id,
            "State: pending → payment_received"
        );

        // Transition: payment_received → processing
        sqlx::query(
            r#"
            UPDATE transactions
            SET status = 'processing',
                updated_at = NOW()
            WHERE transaction_id = $1
              AND status = 'payment_received'
            "#,
        )
        .bind(tx_id)
        .execute((*self.db).as_ref())
        .await?;

        info!(tx_id = %tx_id, "State: payment_received → processing");
        info!(tx_id = %tx_id, "Transaction status updated to processing");
        metrics::inc_payments_confirmed(provider.as_str());

        // Reload the fresh record for the transfer step
        let tx = repo
            .find_by_id(&tx_id.to_string())
            .await?
            .ok_or_else(|| ProcessorError::Internal(format!("Transaction vanished: {}", tx_id)))?;

        if let Err(e) = self
            .audit_mint_action(&tx, MintActionType::MintRequested, json!({
                "stage": "payment_confirmed",
                "status": "processing",
            }))
            .await
        {
            warn!(tx_id = %tx_id, error = %e, "Failed to record MintRequested audit event");
        }

        // Execute the cNGN transfer on Stellar
        self.execute_cngn_transfer(&tx).await
    }

    // =========================================================================
    // Stage 3 — Execute cNGN Transfer on Stellar
    // =========================================================================

    /// Build, sign, and submit the cNGN transfer on Stellar.
    /// Verifies trustline and liquidity before attempting submission.
    /// Retries transient errors with exponential backoff.
    /// On permanent failure, triggers a fiat refund.
    #[instrument(
        skip(self, tx),
        fields(worker = "onramp", tx_id = %tx.transaction_id, wallet = %tx.wallet_address)
    )]
    async fn execute_cngn_transfer(&self, tx: &Transaction) -> Result<(), ProcessorError> {
        info!(
            tx_id = %tx.transaction_id,
            wallet = %tx.wallet_address,
            amount_cngn = %tx.cngn_amount,
            "Executing cNGN transfer on Stellar"
        );

        // Guard: already has a blockchain hash — idempotent no-op
        if tx.blockchain_tx_hash.is_some() {
            info!(
                tx_id = %tx.transaction_id,
                hash = ?tx.blockchain_tx_hash,
                "Stellar tx already submitted — skipping (idempotent)"
            );
            return Ok(());
        }

        if let Err(e) = self
            .audit_mint_action(
                tx,
                MintActionType::MintSubmitted,
                json!({"stage": "execute_cngn_transfer_start"}),
            )
            .await
        {
            warn!(tx_id = %tx.transaction_id, error = %e, "Failed to record MintSubmitted audit event");
        }

        // Pre-flight: verify destination trustline
        if !self.verify_trustline(&tx.wallet_address).await? {
            warn!(
                tx_id = %tx.transaction_id,
                wallet = %tx.wallet_address,
                "Destination has no cNGN trustline — initiating refund"
            );
            self.transition_failed(
                &tx.transaction_id,
                "processing",
                FailureReason::TrustlineNotFound,
                &format!("No cNGN trustline on wallet {}", tx.wallet_address),
            )
            .await?;
            self.initiate_refund(tx, FailureReason::TrustlineNotFound).await?;
            self.initiate_refund(tx, FailureReason::TrustlineNotFound)
                .await?;
            metrics::inc_transfers_failed("trustline_not_found");
            return Ok(());
        }

        // Pre-flight: verify system wallet has sufficient cNGN
        if !self.verify_cngn_liquidity(&tx.cngn_amount).await? {
            warn!(
                tx_id = %tx.transaction_id,
                required = %tx.cngn_amount,
                "Insufficient cNGN liquidity — initiating refund"
            );
            self.transition_failed(
                &tx.transaction_id,
                "processing",
                FailureReason::InsufficientCngnBalance,
                &format!("System wallet has insufficient cNGN for {}", tx.cngn_amount),
            )
            .await?;
            self.initiate_refund(tx, FailureReason::InsufficientCngnBalance).await?;
            self.initiate_refund(tx, FailureReason::InsufficientCngnBalance)
                .await?;
            metrics::inc_transfers_failed("insufficient_balance");
            return Ok(());
        }

        // Submit with retry + exponential backoff
        match self.submit_cngn_transfer_with_retry(tx).await {
            Ok(stellar_hash) => {
                info!(
                    tx_id = %tx.transaction_id,
                    stellar_hash = %stellar_hash,
                    "cNGN transfer submitted to Stellar — awaiting confirmation"
                );

                // Persist the hash immediately so it's recoverable on crash
                sqlx::query(
                    r#"
                    UPDATE transactions
                    SET blockchain_tx_hash = $1,
                        metadata = metadata || $2,
                        updated_at = NOW()
                    WHERE transaction_id = $3
                      AND status = 'processing'
                    "#,
                )
                .bind(&stellar_hash)
                .bind(json!({
                    "stellar_submitted_at": Utc::now().to_rfc3339(),
                    "stellar_tx_hash": stellar_hash,
                }))
                .bind(&tx.transaction_id)
                .execute((*self.db).as_ref())
                .await?;

                info!(
                    tx_id = %tx.transaction_id,
                    stellar_hash = %stellar_hash,
                    "Stellar tx hash persisted — monitoring worker will confirm"
                );
                metrics::inc_transfers_submitted();
            }
            Err(e) => {
                error!(
                    tx_id = %tx.transaction_id,
                    error = %e,
                    "All Stellar submission attempts exhausted — initiating refund"
                );

                if let Err(audit_err) = self
                    .audit_mint_action(
                        tx,
                        MintActionType::MintFailed,
                        json!({"reason": e.to_string(), "stage": "submit_failed"}),
                    )
                    .await
                {
                    warn!(tx_id = %tx.transaction_id, error = %audit_err, "Failed to record MintFailed audit event");
                }

                let reason = if e.is_transient() {
                    FailureReason::StellarTransientError
                } else {
                    FailureReason::StellarPermanentError
                };
                self.transition_failed(
                    &tx.transaction_id,
                    "processing",
                    reason.clone(),
                    &e.to_string(),
                )
                .await?;
                self.initiate_refund(tx, reason).await?;

                // Determine if transient or permanent error
                match e {
                    ProcessorError::StellarTransientError(_) => {
                        warn!("Transient Stellar error, will retry on next cycle");
                        // Leave in processing state for retry
                    }
                    _ => {
                        // Permanent error - mark failed and initiate refund
                        self.mark_transaction_failed(
                            &tx.transaction_id,
                            FailureReason::StellarPermanentError,
                            &e.to_string(),
                        )
                        .await?;
                        self.initiate_refund(tx, FailureReason::StellarPermanentError)
                            .await?;
                        metrics::inc_transfers_failed("stellar_error");
                    }
                }
            }
        }

        Ok(())
    }

    /// Verify that the destination wallet has a cNGN trustline.
    async fn verify_trustline(&self, wallet_address: &str) -> Result<bool, ProcessorError> {
        debug!(wallet = %wallet_address, "Verifying cNGN trustline");
        let manager = CngnTrustlineManager::new((*self.stellar).clone());
        let status = manager
            .check_trustline(wallet_address)
            .await
            .map_err(ProcessorError::from)?;
        Ok(status.has_trustline && status.is_authorized)
    }

    /// Verify the system wallet has enough cNGN to cover the transfer.
    async fn verify_cngn_liquidity(&self, required: &BigDecimal) -> Result<bool, ProcessorError> {
        debug!(required = %required, "Verifying cNGN liquidity");
        let balance_str = self
            .stellar
            .get_cngn_balance(&self.config.system_wallet_address, None)
            .await
            .map_err(ProcessorError::from)?;

        let available = match balance_str {
            Some(b) => BigDecimal::from_str(&b).unwrap_or_default(),
            None => BigDecimal::from(0),
        };

        if &available < required {
            warn!(
                available = %available,
                required = %required,
                "Insufficient cNGN in system wallet"
            );
            return Ok(false);
        }
        Ok(true)
    }

    async fn audit_mint_action(
        &self,
        tx: &Transaction,
        action_type: MintActionType,
        request_payload: serde_json::Value,
    ) -> Result<(), String> {
        if let Some(service) = &self.mint_audit_service {
            let payload = json!({
                "transaction_id": tx.transaction_id.to_string(),
                "wallet_address": tx.wallet_address,
                "amount_cngn": tx.cngn_amount.to_string(),
                "status": tx.status,
                "action": action_type.as_str(),
                "request_payload": request_payload,
            });

            service
                .record_event(
                    &self.config.system_wallet_address,
                    &self.config.system_wallet_address,
                    action_type,
                    payload,
                )
                .await
        } else {
            Ok(())
        }
    }

    /// Submit the cNGN transfer with exponential backoff retry for transient errors.
    /// Returns the Stellar transaction hash on success.
    async fn submit_cngn_transfer_with_retry(
        &self,
        tx: &Transaction,
    ) -> Result<String, ProcessorError> {
        let mut last_err: Option<ProcessorError> = None;

        for attempt in 0..=self.config.stellar_max_retries {
            if attempt > 0 {
                let backoff = self
                    .config
                    .stellar_retry_backoff_secs
                    .get((attempt - 1) as usize)
                    .copied()
                    .unwrap_or(8);
                warn!(
                    tx_id = %tx.transaction_id,
                    attempt = attempt,
                    backoff_secs = backoff,
                    "Retrying Stellar submission after transient error"
                );
                sleep(Duration::from_secs(backoff)).await;
            }

            match self.attempt_single_cngn_transfer(tx).await {
                Ok(hash) => return Ok(hash),
                Err(e) => {
                    if e.is_transient() {
                        warn!(
                            tx_id = %tx.transaction_id,
                            attempt = attempt,
                            error = %e,
                            "Transient Stellar error"
                        );
                        last_err = Some(e);
                        // continue to next retry
                    } else {
                        // Permanent error — fail fast, no more retries
                        error!(
                            tx_id = %tx.transaction_id,
                            error = %e,
                            "Permanent Stellar error — aborting retries"
                        );
                        return Err(e);
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ProcessorError::StellarTransientError("max retries exceeded".to_string())
        }))
    }

    /// Build, sign, and submit a single cNGN payment transaction.
    async fn attempt_single_cngn_transfer(
        &self,
        tx: &Transaction,
    ) -> Result<String, ProcessorError> {
        let builder = CngnPaymentBuilder::new((*self.stellar).clone());

        // Encode the transaction ID in the memo for on-chain traceability
        let memo = CngnMemo::Text(format!("onramp:{}", tx.transaction_id));

        // Build the unsigned transaction draft
        let draft = builder
            .build_payment(
                &self.config.system_wallet_address,
                &tx.wallet_address,
                &tx.cngn_amount.to_string(),
                memo,
                None,
            )
            .await
            .map_err(ProcessorError::from)?;

        // Sign with the system wallet secret
        let signed = builder
            .sign_payment(draft, &self.config.system_wallet_secret)
            .map_err(ProcessorError::from)?;

        // Submit to Stellar Horizon
        let result = builder
            .submit_signed_payment(&signed.signed_envelope_xdr)
            .await
            .map_err(ProcessorError::from)?;

        // Extract the transaction hash from the Horizon response
        let hash = result
            .get("hash")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                ProcessorError::StellarPermanentError(
                    "Horizon response missing 'hash' field".to_string(),
                )
            })?;

        Ok(hash)
    }

    // =========================================================================
    // Stage 4 — Monitor Stellar Confirmations
    // =========================================================================

    /// Poll Stellar for confirmation of submitted transactions.
    /// On confirmation, transition to completed and emit a completion event.
    /// On absolute timeout, trigger a refund.
    #[instrument(skip(self), fields(worker = "onramp"))]
    async fn monitor_stellar_confirmations(&self) -> Result<(), ProcessorError> {
        let processing: Vec<Transaction> = sqlx::query_as::<_, Transaction>(
            r#"
            SELECT transaction_id, wallet_address, type, from_currency, to_currency,
                   from_amount, to_amount, cngn_amount, status, payment_provider,
                   payment_reference, blockchain_tx_hash, error_message, metadata,
                   created_at, updated_at
            FROM transactions
            WHERE type = 'onramp'
              AND status = 'processing'
              AND blockchain_tx_hash IS NOT NULL
            ORDER BY updated_at ASC
            LIMIT 50
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .fetch_all((*self.db).as_ref())
        .await?;

        for tx in processing {
            let hash = match tx.blockchain_tx_hash.as_deref() {
                Some(h) => h.to_string(),
                None => continue,
            };

            // Check absolute confirmation timeout
            let age_mins = Utc::now()
                .signed_duration_since(tx.updated_at)
                .num_minutes() as u64;

            if age_mins > self.config.stellar_confirmation_timeout_mins {
                error!(
                    tx_id = %tx.transaction_id,
                    stellar_hash = %hash,
                    age_mins = age_mins,
                    "Stellar confirmation timeout — initiating refund"
                );
                self.transition_failed(
                    &tx.transaction_id,
                    "processing",
                    FailureReason::StellarTransientError,
                    &format!(
                        "Stellar confirmation timed out after {} minutes",
                        age_mins
                    ),
                )
                .await?;
                self.initiate_refund(&tx, FailureReason::StellarTransientError).await?;
                continue;
            }

            match self.check_stellar_confirmation(&hash).await {
                Ok(Some(true)) => {
                    info!(
                        tx_id = %tx.transaction_id,
                        stellar_hash = %hash,
                        "Stellar transaction confirmed on-chain"
                    );
                    self.complete_transaction(&tx.transaction_id, &hash).await?;
                }
                Ok(Some(false)) => {
                    // Transaction found but failed on-chain
                    error!(
                        tx_id = %tx.transaction_id,
                        stellar_hash = %hash,
                        "Stellar transaction failed on-chain — initiating refund"
                    );
                    self.transition_failed(
                        &tx.transaction_id,
                        "processing",
                        FailureReason::StellarPermanentError,
                        "Stellar transaction failed on-chain",
                    )
                    .await?;
                    self.initiate_refund(&tx, FailureReason::StellarPermanentError).await?;
                }
                Ok(None) => {
                    // Not yet visible on Horizon — still pending
                    debug!(
                        tx_id = %tx.transaction_id,
                        stellar_hash = %hash,
                        "Stellar transaction not yet confirmed"
                    );
                }
                Err(e) => {
                    warn!(
                        tx_id = %tx.transaction_id,
                        stellar_hash = %hash,
                        error = %e,
                        "Error checking Stellar confirmation"
                    );
                }
            }
        }

        Ok(())
    }

    /// Query Stellar Horizon for a transaction by hash.
    /// Returns:
    ///   - `Ok(Some(true))`  — found and successful
    ///   - `Ok(Some(false))` — found but failed
    ///   - `Ok(None)`        — not yet visible (still pending)
    ///   - `Err(_)`          — network/transient error
    async fn check_stellar_confirmation(
        &self,
        tx_hash: &str,
    ) -> Result<Option<bool>, ProcessorError> {
        match self.stellar.get_transaction_by_hash(tx_hash).await {
            Ok(record) => Ok(Some(record.successful)),
            Err(StellarError::TransactionFailed { .. }) => {
                // Horizon returned 404 — not yet visible
                Ok(None)
            }
            Err(e) => Err(ProcessorError::from(e)),
        }
    }

    // =========================================================================
    // State Transition Helpers
    // =========================================================================

    /// Transition a transaction to `completed` and emit a completion event.
    /// Idempotent — safe to call if already completed.
    async fn complete_transaction(
        &self,
        tx_id: &Uuid,
        stellar_hash: &str,
    ) -> Result<(), ProcessorError> {
        let rows = sqlx::query(
            r#"
            UPDATE transactions
            SET status = 'completed',
                metadata = metadata || $1,
                updated_at = NOW()
            WHERE transaction_id = $2
              AND status = 'processing'
            "#,
        )
        .bind(json!({
            "completed_at": Utc::now().to_rfc3339(),
            "stellar_confirmed_hash": stellar_hash,
        }))
        .bind(tx_id)
        .execute((*self.db).as_ref())
        .await?
        .rows_affected();

        if rows > 0 {
            info!(
                tx_id = %tx_id,
                stellar_hash = %stellar_hash,
                "State: processing → completed ✅"
            );
            // Emit completion event for downstream consumers (notifications, analytics)
            self.emit_completion_event(tx_id, stellar_hash).await;
        }

        Ok(())
    }

    /// Transition a transaction to `failed` using optimistic locking.
    /// `expected_status` is the status we expect the row to be in — prevents
    /// overwriting a later state if another process already advanced it.
    async fn transition_failed(
        &self,
        tx_id: &Uuid,
        expected_status: &str,
        reason: FailureReason,
        message: &str,
    ) -> Result<(), ProcessorError> {
        let rows = sqlx::query(
            r#"
            UPDATE transactions
            SET status = 'failed',
                error_message = $1,
                metadata = metadata || $2,
                updated_at = NOW()
            WHERE transaction_id = $3
              AND status = $4
            "#,
        )
        .bind(message)
        .bind(json!({
            "failure_reason": reason.as_str(),
            "failed_at": Utc::now().to_rfc3339(),
            "failure_message": message,
        }))
        .bind(tx_id)
        .bind(expected_status)
        .execute((*self.db).as_ref())
        .await?
        .rows_affected();

        if rows > 0 {
            warn!(
                tx_id = %tx_id,
                reason = %reason.as_str(),
                message = %message,
                "State: {} → failed ❌",
                expected_status
            );
        }

        Ok(())
    }

    /// Emit a structured completion event. In production this would publish to
    /// an event bus (SNS/SQS/Kafka). For now it logs a structured record that
    /// downstream consumers can tail.
    async fn emit_completion_event(&self, tx_id: &Uuid, stellar_hash: &str) {
        info!(
            event = "onramp.completed",
            tx_id = %tx_id,
            stellar_hash = %stellar_hash,
            timestamp = %Utc::now().to_rfc3339(),
            "🎉 Onramp transaction completed"
        );
    }

    // =========================================================================
    // Refund Flow
    // =========================================================================

    /// Initiate a fiat refund via the payment provider.
    /// Retries with exponential backoff up to `refund_max_retries`.
    /// If all refund attempts fail, transitions to `refund_failed` and logs
    /// a critical alert for manual ops intervention.
    ///
    /// State transition: failed → refund_initiated → refunded
    #[instrument(
        skip(self, tx),
        fields(worker = "onramp", tx_id = %tx.transaction_id)
    )]
    async fn initiate_refund(
        &self,
        tx: &Transaction,
        reason: FailureReason,
    ) -> Result<(), ProcessorError> {
        info!(
            tx_id = %tx.transaction_id,
            provider = ?tx.payment_provider,
            reason = %reason.as_str(),
            amount_ngn = %tx.from_amount,
            "Initiating fiat refund"
        );

        // Transition: failed → refund_initiated
        sqlx::query(
            r#"
            UPDATE transactions
            SET status = 'refund_initiated',
                metadata = metadata || $1,
                updated_at = NOW()
            WHERE transaction_id = $2
              AND status = 'failed'
            "#,
        )
        .bind(json!({
            "refund_initiated_at": Utc::now().to_rfc3339(),
            "refund_reason": reason.as_str(),
        }))
        .bind(&tx.transaction_id)
        .execute((*self.db).as_ref())
        .await?;
        // TODO: Call PaymentOrchestrator to initiate refund
        // For now, just log
        metrics::inc_refunds_initiated(tx.payment_provider.as_deref().unwrap_or("unknown"));

        // Attempt refund with retry
        let mut last_err: Option<String> = None;

        for attempt in 0..=self.config.refund_max_retries {
            if attempt > 0 {
                let backoff = self
                    .config
                    .refund_retry_backoff_secs
                    .get((attempt - 1) as usize)
                    .copied()
                    .unwrap_or(120);
                warn!(
                    tx_id = %tx.transaction_id,
                    attempt = attempt,
                    backoff_secs = backoff,
                    "Retrying refund"
                );
                sleep(Duration::from_secs(backoff)).await;
            }

            match self.attempt_provider_refund(tx).await {
                Ok(refund_ref) => {
                    // Transition: refund_initiated → refunded
                    sqlx::query(
                        r#"
                        UPDATE transactions
                        SET status = 'refunded',
                            metadata = metadata || $1,
                            updated_at = NOW()
                        WHERE transaction_id = $2
                          AND status = 'refund_initiated'
                        "#,
                    )
                    .bind(json!({
                        "refunded_at": Utc::now().to_rfc3339(),
                        "refund_reference": refund_ref,
                    }))
                    .bind(&tx.transaction_id)
                    .execute((*self.db).as_ref())
                    .await?;

                    info!(
                        tx_id = %tx.transaction_id,
                        refund_ref = %refund_ref,
                        "State: refund_initiated → refunded ✅"
                    );
                    return Ok(());
                }
                Err(e) => {
                    warn!(
                        tx_id = %tx.transaction_id,
                        attempt = attempt,
                        error = %e,
                        "Refund attempt failed"
                    );
                    last_err = Some(e.to_string());
                }
            }
        }

        // All refund attempts exhausted — requires manual intervention
        let err_msg = last_err.unwrap_or_else(|| "unknown refund error".to_string());
        error!(
            tx_id = %tx.transaction_id,
            error = %err_msg,
            amount_ngn = %tx.from_amount,
            provider = ?tx.payment_provider,
            "🚨 CRITICAL: All refund attempts failed — manual intervention required"
        );

        sqlx::query(
            r#"
            UPDATE transactions
            SET status = 'refund_failed',
                error_message = $1,
                metadata = metadata || $2,
                updated_at = NOW()
            WHERE transaction_id = $3
              AND status = 'refund_initiated'
            "#,
        )
        .bind(&err_msg)
        .bind(json!({
            "refund_failed_at": Utc::now().to_rfc3339(),
            "refund_failure_message": err_msg,
            "requires_manual_review": true,
        }))
        .bind(&tx.transaction_id)
        .execute((*self.db).as_ref())
        .await?;

        Err(ProcessorError::RefundFailed(err_msg))
    }

    /// Attempt a single refund via the payment provider.
    /// Returns the provider's refund reference on success.
    async fn attempt_provider_refund(&self, tx: &Transaction) -> Result<String, ProcessorError> {
        let provider_name = tx
            .payment_provider
            .as_deref()
            .ok_or_else(|| ProcessorError::RefundFailed("No payment provider on transaction".to_string()))?;

        let provider_ref = tx
            .payment_reference
            .as_deref()
            .ok_or_else(|| ProcessorError::RefundFailed("No payment reference on transaction".to_string()))?;

        let provider_enum: ProviderName = provider_name
            .parse()
            .map_err(|e: crate::payments::error::PaymentError| {
                ProcessorError::RefundFailed(e.to_string())
            })?;

        let provider = self
            .provider_factory
            .get_provider(provider_enum)
            .map_err(|e| ProcessorError::RefundFailed(e.to_string()))?;

        // Use process_withdrawal as the refund mechanism — send fiat back to user
        let withdrawal_req = crate::payments::types::WithdrawalRequest {
            amount: crate::payments::types::Money {
                amount: tx.from_amount.to_string(),
                currency: tx.from_currency.clone(),
            },
            recipient: crate::payments::types::WithdrawalRecipient {
                account_name: None,
                account_number: None,
                bank_code: None,
                phone_number: None,
            },
            withdrawal_method: crate::payments::types::WithdrawalMethod::BankTransfer,
            transaction_reference: format!("refund:{}", tx.transaction_id),
            reason: Some(format!("Onramp refund for transaction {}", tx.transaction_id)),
            metadata: Some(json!({
                "original_reference": provider_ref,
                "refund_type": "onramp_failure",
            })),
        };

        let resp = provider
            .process_withdrawal(withdrawal_req)
            .await
            .map_err(|e| ProcessorError::RefundFailed(e.to_string()))?;

        let refund_ref = resp
            .provider_reference
            .unwrap_or_else(|| format!("refund:{}", tx.transaction_id));

        Ok(refund_ref)
    }
}

// ============================================================================
// Metrics (real Prometheus counters via crate::metrics)
// ============================================================================

mod metrics {
    #[inline]
    pub fn inc_payments_failed(reason: &str) {
        crate::metrics::cngn::transactions_total()
            .with_label_values(&["onramp", "failed"])
            .inc();
        crate::metrics::worker::errors_total()
            .with_label_values(&["onramp_processor", reason])
            .inc();
    }

    #[inline]
    pub fn inc_payments_confirmed(provider: &str) {
        crate::metrics::cngn::transactions_total()
            .with_label_values(&["onramp", "completed"])
            .inc();
        let _ = provider; // label available on payment metrics
    }

    #[inline]
    pub fn inc_transfers_submitted() {
        crate::metrics::stellar::tx_submissions_total()
            .with_label_values(&["success"])
            .inc();
    }

    #[inline]
    pub fn inc_transfers_confirmed() {
        // already counted via cngn transactions_total completed
    }

    #[inline]
    pub fn inc_transfers_failed(reason: &str) {
        crate::metrics::stellar::tx_submissions_total()
            .with_label_values(&["failed"])
            .inc();
        crate::metrics::worker::errors_total()
            .with_label_values(&["onramp_processor", reason])
            .inc();
    }

    #[inline]
    pub fn inc_refunds_initiated(provider: &str) {
        crate::metrics::cngn::transactions_total()
            .with_label_values(&["onramp", "refunded"])
            .inc();
        let _ = provider;
    }
}
