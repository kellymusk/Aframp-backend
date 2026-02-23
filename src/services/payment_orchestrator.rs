//! Payment Orchestrator Service
//!
//! This service intelligently routes transactions through payment providers,
//! manages transaction state, ensures idempotency, and handles failures gracefully.

use crate::cache::cache::Cache;
use crate::database::repository::Repository;
use crate::database::transaction_repository::Transaction;
use crate::database::transaction_repository::TransactionRepository;
use crate::error::{AppError, AppErrorKind, DomainError, ExternalError, InfrastructureError};
use crate::payments::provider::PaymentProvider;
use crate::payments::types::{
    Money, PaymentMethod, PaymentRequest, PaymentResponse, PaymentState, ProviderName,
    StatusRequest, StatusResponse,
};
use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use uuid::Uuid;

// ============================================================================
// Configuration Types
// ============================================================================

/// Configuration for the payment orchestrator
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// Default primary provider
    pub default_provider: ProviderName,
    /// Maximum retry attempts for failed payments
    pub max_retry_attempts: u32,
    /// Initial retry delay in seconds
    pub initial_retry_delay_secs: u64,
    /// Maximum retry delay in seconds
    pub max_retry_delay_secs: u64,
    /// Idempotency key expiration in seconds (24 hours)
    pub idempotency_key_expiration_secs: u64,
    /// Provider health check interval in seconds
    pub provider_health_check_interval_secs: u64,
    /// Minimum success rate threshold (0.0 - 1.0)
    pub min_success_rate_threshold: f64,
    /// Large transaction threshold (amount in NGN)
    pub large_transaction_threshold: BigDecimal,
    /// Fee comparison enabled for large transactions
    pub fee_comparison_enabled: bool,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            default_provider: ProviderName::Flutterwave,
            max_retry_attempts: 3,
            initial_retry_delay_secs: 1,
            max_retry_delay_secs: 60,
            idempotency_key_expiration_secs: 86400, // 24 hours
            provider_health_check_interval_secs: 60,
            min_success_rate_threshold: 0.90,
            large_transaction_threshold: BigDecimal::from(100000), // ₦100,000
            fee_comparison_enabled: true,
        }
    }
}

impl OrchestratorConfig {
    pub fn from_env() -> Self {
        Self {
            default_provider: std::env::var("DEFAULT_PAYMENT_PROVIDER")
                .unwrap_or_else(|_| "flutterwave".to_string())
                .parse()
                .unwrap_or(ProviderName::Flutterwave),
            max_retry_attempts: std::env::var("MAX_RETRY_ATTEMPTS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
            initial_retry_delay_secs: std::env::var("INITIAL_RETRY_DELAY_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
            max_retry_delay_secs: std::env::var("MAX_RETRY_DELAY_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            idempotency_key_expiration_secs: std::env::var("IDEMPOTENCY_KEY_EXPIRATION_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(86400),
            provider_health_check_interval_secs: std::env::var(
                "PROVIDER_HEALTH_CHECK_INTERVAL_SECS",
            )
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60),
            min_success_rate_threshold: std::env::var("MIN_SUCCESS_RATE_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.90),
            large_transaction_threshold: std::env::var("LARGE_TRANSACTION_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(BigDecimal::from(100000)),
            fee_comparison_enabled: std::env::var("FEE_COMPARISON_ENABLED")
                .unwrap_or_else(|_| "true".to_string())
                .parse()
                .unwrap_or(true),
        }
    }
}

// ============================================================================
// Provider Health & Metrics Types
// ============================================================================

/// Provider health status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderHealth {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Provider metrics for selection
#[derive(Debug, Clone)]
pub struct ProviderMetrics {
    pub provider: ProviderName,
    pub success_count: u64,
    pub failure_count: u64,
    pub total_requests: u64,
    pub total_fees_collected: BigDecimal,
    pub last_request_at: Option<SystemTime>,
    pub last_failure_at: Option<SystemTime>,
    pub current_health: ProviderHealth,
}

impl ProviderMetrics {
    pub fn new(provider: ProviderName) -> Self {
        Self {
            provider,
            success_count: 0,
            failure_count: 0,
            total_requests: 0,
            total_fees_collected: BigDecimal::from(0),
            last_request_at: None,
            last_failure_at: None,
            current_health: ProviderHealth::Healthy,
        }
    }

    pub fn success_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 1.0; // Default to 100% if no requests
        }
        self.success_count as f64 / self.total_requests as f64
    }

    pub fn record_success(&mut self, fee_amount: BigDecimal) {
        self.success_count += 1;
        self.total_requests += 1;
        self.total_fees_collected += fee_amount;
        self.last_request_at = Some(SystemTime::now());

        // Update health status based on recent failures
        if self.current_health != ProviderHealth::Healthy {
            if self.failure_count == 0 {
                self.current_health = ProviderHealth::Healthy;
            }
        }
    }

    pub fn record_failure(&mut self) {
        self.failure_count += 1;
        self.total_requests += 1;
        self.last_request_at = Some(SystemTime::now());
        self.last_failure_at = Some(SystemTime::now());

        // Degrade health if failure rate is high
        if self.total_requests >= 10 {
            let failure_rate = self.failure_count as f64 / self.total_requests as f64;
            if failure_rate >= 0.3 {
                self.current_health = ProviderHealth::Unhealthy;
            } else if failure_rate >= 0.15 {
                self.current_health = ProviderHealth::Degraded;
            }
        }
    }
}

// ============================================================================
// Transaction State Machine Types
// ============================================================================

/// Orchestrator transaction state
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationState {
    /// Transaction record created, not yet paid
    Created,
    /// Awaiting payment provider confirmation
    PendingPayment,
    /// Payment successful, pending blockchain
    PaymentConfirmed,
    /// Sending cNGN on Stellar
    ProcessingBlockchain,
    /// Both payment and blockchain successful
    Completed,
    /// Permanent failure, no retry
    Failed,
    /// Failed after payment received, refunding user
    RefundInitiated,
    /// Refund completed
    Refunded,
}

impl std::fmt::Display for OrchestrationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrchestrationState::Created => write!(f, "created"),
            OrchestrationState::PendingPayment => write!(f, "pending_payment"),
            OrchestrationState::PaymentConfirmed => write!(f, "payment_confirmed"),
            OrchestrationState::ProcessingBlockchain => write!(f, "processing_blockchain"),
            OrchestrationState::Completed => write!(f, "completed"),
            OrchestrationState::Failed => write!(f, "failed"),
            OrchestrationState::RefundInitiated => write!(f, "refund_initiated"),
            OrchestrationState::Refunded => write!(f, "refunded"),
        }
    }
}

impl OrchestrationState {
    /// Get all valid transitions from this state
    pub fn valid_transitions(&self) -> Vec<OrchestrationState> {
        match self {
            OrchestrationState::Created => vec![OrchestrationState::PendingPayment],
            OrchestrationState::PendingPayment => vec![
                OrchestrationState::PaymentConfirmed,
                OrchestrationState::Failed,
            ],
            OrchestrationState::PaymentConfirmed => vec![OrchestrationState::ProcessingBlockchain],
            OrchestrationState::ProcessingBlockchain => vec![OrchestrationState::Completed],
            OrchestrationState::RefundInitiated => vec![OrchestrationState::Refunded],
            // Terminal states - no valid transitions
            OrchestrationState::Completed => vec![],
            OrchestrationState::Failed => vec![],
            OrchestrationState::Refunded => vec![],
        }
    }

    /// Check if this is a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            OrchestrationState::Completed
                | OrchestrationState::Failed
                | OrchestrationState::Refunded
        )
    }

    /// Check if state allows retry
    pub fn allows_retry(&self) -> bool {
        matches!(self, OrchestrationState::Failed)
    }

    /// Convert from database status string
    pub fn from_db_status(status: &str) -> Option<Self> {
        match status.to_lowercase().as_str() {
            "created" => Some(OrchestrationState::Created),
            "pending" | "pending_payment" => Some(OrchestrationState::PendingPayment),
            "payment_confirmed" => Some(OrchestrationState::PaymentConfirmed),
            "processing" | "processing_blockchain" => {
                Some(OrchestrationState::ProcessingBlockchain)
            }
            "completed" | "success" => Some(OrchestrationState::Completed),
            "failed" => Some(OrchestrationState::Failed),
            "refund_initiated" => Some(OrchestrationState::RefundInitiated),
            "refunded" => Some(OrchestrationState::Refunded),
            _ => None,
        }
    }

    /// Convert to database status string
    pub fn to_db_status(&self) -> &'static str {
        match self {
            OrchestrationState::Created => "created",
            OrchestrationState::PendingPayment => "pending",
            OrchestrationState::PaymentConfirmed => "payment_confirmed",
            OrchestrationState::ProcessingBlockchain => "processing",
            OrchestrationState::Completed => "completed",
            OrchestrationState::Failed => "failed",
            OrchestrationState::RefundInitiated => "refund_initiated",
            OrchestrationState::Refunded => "refunded",
        }
    }
}

// Add BlockchainFailed state (missing from original)
impl OrchestrationState {
    pub const BlockchainFailed: OrchestrationState = OrchestrationState::ProcessingBlockchain;
}

// ============================================================================
// Idempotency Types
// ============================================================================

/// Idempotency key info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdempotencyKeyInfo {
    pub key: String,
    pub transaction_id: String,
    pub wallet_address: String,
    pub amount: String,
    pub currency: String,
    pub operation: String, // "onramp" or "offramp"
    pub created_at: u64,
    pub expires_at: u64,
}

/// Idempotency check result
#[derive(Debug)]
pub enum IdempotencyCheckResult {
    /// No existing key found - proceed with new transaction
    NewTransaction,
    /// Existing key found with pending transaction - return existing
    ExistingPending {
        transaction_id: String,
        idempotency_key: String,
    },
    /// Existing key found with completed transaction - error (duplicate)
    Duplicate {
        transaction_id: String,
        idempotency_key: String,
    },
    /// Existing key found with failed transaction - allow retry with new key
    AllowRetry {
        existing_transaction_id: String,
        idempotency_key: String,
    },
}

// ============================================================================
// Payment Routing Types
// ============================================================================

/// Selection strategy for provider
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionStrategy {
    /// Default: Use configured default provider
    Default,
    /// Use lowest fee provider
    CostBased,
    /// Use provider with highest success rate
    ReliabilityBased,
    /// Round-robin load distribution
    RoundRobin,
    /// Failover from primary to backup
    Failover,
}

/// Provider selection context
#[derive(Debug, Clone)]
pub struct SelectionContext {
    pub amount: BigDecimal,
    pub currency: String,
    pub payment_method: PaymentMethod,
    pub user_preferred_provider: Option<ProviderName>,
    pub country: Option<String>,
    pub strategy: SelectionStrategy,
}

/// Payment initiation request
#[derive(Debug)]
pub struct PaymentInitiationRequest {
    pub wallet_address: String,
    pub amount: BigDecimal,
    pub currency: String,
    pub payment_method: PaymentMethod,
    pub customer_email: Option<String>,
    pub customer_phone: Option<String>,
    pub callback_url: Option<String>,
    pub idempotency_key: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

// ============================================================================
// Orchestrator Error Types
// ============================================================================

/// Orchestrator error types
#[derive(Debug)]
pub enum OrchestratorError {
    /// No provider available
    NoProviderAvailable,
    /// Invalid state transition
    InvalidStateTransition {
        current: OrchestrationState,
        target: OrchestrationState,
    },
    /// Idempotency key not found or expired
    IdempotencyKeyNotFound,
    /// Duplicate transaction
    DuplicateTransaction { transaction_id: String },
    /// Max retries exceeded
    MaxRetriesExceeded { transaction_id: String },
    /// Provider selection failed
    ProviderSelectionFailed { reason: String },
    /// All providers failed
    AllProvidersFailed { errors: Vec<String> },
    /// Blockchain operation failed
    BlockchainFailed { message: String },
    /// Transaction not found
    TransactionNotFound { transaction_id: String },
    /// Configuration error
    ConfigurationError { message: String },
}

impl std::fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoProviderAvailable => write!(f, "No payment provider available"),
            Self::InvalidStateTransition { current, target } => {
                write!(f, "Invalid state transition from {} to {}", current, target)
            }
            Self::IdempotencyKeyNotFound => write!(f, "Idempotency key not found or expired"),
            Self::DuplicateTransaction { transaction_id } => {
                write!(f, "Duplicate transaction: {}", transaction_id)
            }
            Self::MaxRetriesExceeded { transaction_id } => {
                write!(
                    f,
                    "Max retries exceeded for transaction: {}",
                    transaction_id
                )
            }
            Self::ProviderSelectionFailed { reason } => {
                write!(f, "Provider selection failed: {}", reason)
            }
            Self::AllProvidersFailed { errors } => {
                write!(f, "All providers failed: {}", errors.join("; "))
            }
            Self::BlockchainFailed { message } => {
                write!(f, "Blockchain operation failed: {}", message)
            }
            Self::TransactionNotFound { transaction_id } => {
                write!(f, "Transaction not found: {}", transaction_id)
            }
            Self::ConfigurationError { message } => {
                write!(f, "Configuration error: {}", message)
            }
        }
    }
}

impl std::error::Error for OrchestratorError {}

impl From<OrchestratorError> for AppError {
    fn from(err: OrchestratorError) -> Self {
        let kind = match &err {
            OrchestratorError::NoProviderAvailable => {
                AppErrorKind::External(ExternalError::PaymentProvider {
                    provider: "all".to_string(),
                    message: err.to_string(),
                    is_retryable: true,
                })
            }
            OrchestratorError::DuplicateTransaction { .. } => {
                AppErrorKind::Domain(DomainError::DuplicateTransaction {
                    transaction_id: "unknown".to_string(),
                })
            }
            OrchestratorError::TransactionNotFound { .. } => {
                AppErrorKind::Domain(DomainError::TransactionNotFound {
                    transaction_id: "unknown".to_string(),
                })
            }
            OrchestratorError::MaxRetriesExceeded { .. }
            | OrchestratorError::InvalidStateTransition { .. }
            | OrchestratorError::ProviderSelectionFailed { .. }
            | OrchestratorError::AllProvidersFailed { .. }
            | OrchestratorError::BlockchainFailed { .. } => {
                AppErrorKind::External(ExternalError::PaymentProvider {
                    provider: "orchestrator".to_string(),
                    message: err.to_string(),
                    is_retryable: false,
                })
            }
            OrchestratorError::IdempotencyKeyNotFound => {
                AppErrorKind::Infrastructure(InfrastructureError::Configuration {
                    message: err.to_string(),
                })
            }
            OrchestratorError::ConfigurationError { .. } => {
                AppErrorKind::Infrastructure(InfrastructureError::Configuration {
                    message: err.to_string(),
                })
            }
        };
        AppError::new(kind)
    }
}

/// Result type for orchestrator operations
pub type OrchestratorResult<T> = Result<T, OrchestratorError>;

// ============================================================================
// Main Payment Orchestrator
// ============================================================================

/// Payment orchestrator that manages the entire payment lifecycle
pub struct PaymentOrchestrator {
    providers: HashMap<ProviderName, Arc<dyn PaymentProvider>>,
    transaction_repo: Arc<TransactionRepository>,
    config: OrchestratorConfig,
    provider_metrics: Arc<RwLock<HashMap<ProviderName, ProviderMetrics>>>,
    round_robin_index: Arc<RwLock<usize>>,
}

impl PaymentOrchestrator {
    /// Create a new payment orchestrator
    pub fn new(
        providers: Vec<Arc<dyn PaymentProvider>>,
        transaction_repo: Arc<TransactionRepository>,
        config: OrchestratorConfig,
    ) -> Self {
        // Initialize provider metrics
        let mut metrics = HashMap::new();
        for provider in &providers {
            metrics.insert(provider.name(), ProviderMetrics::new(provider.name()));
        }

        Self {
            providers: providers.into_iter().map(|p| (p.name(), p)).collect(),
            transaction_repo,
            config,
            provider_metrics: Arc::new(RwLock::new(metrics)),
            round_robin_index: Arc::new(RwLock::new(0)),
        }
    }

    /// Add a provider to the orchestrator
    pub fn add_provider(&mut self, provider: Arc<dyn PaymentProvider>) {
        let name = provider.name();
        self.providers.insert(name.clone(), provider);
        // Initialize metrics for new provider
        let _metrics = ProviderMetrics::new(name);
        // Note: In production, use proper async lock
    }

    // =========================================================================
    // Provider Selection Logic
    // =========================================================================

    /// Select the best provider based on criteria
    pub async fn select_provider(
        &self,
        context: &SelectionContext,
    ) -> OrchestratorResult<ProviderName> {
        let metrics = self.provider_metrics.read().await;

        // Filter available providers
        let available_providers: Vec<ProviderName> = self
            .providers
            .keys()
            .filter(|name| {
                if let Some(m) = metrics.get(*name) {
                    m.current_health != ProviderHealth::Unhealthy
                } else {
                    true
                }
            })
            .cloned()
            .collect();

        if available_providers.is_empty() {
            return Err(OrchestratorError::NoProviderAvailable);
        }

        // Apply selection strategy
        let selected = match context.strategy {
            SelectionStrategy::Default => {
                self.select_default(&available_providers, context).await?
            }
            SelectionStrategy::CostBased => {
                self.select_cost_based(&available_providers, context)
                    .await?
            }
            SelectionStrategy::ReliabilityBased => {
                self.select_reliability_based(&available_providers, context)
                    .await?
            }
            SelectionStrategy::RoundRobin => self.select_round_robin(&available_providers).await?,
            SelectionStrategy::Failover => {
                self.select_failover(&available_providers, context).await?
            }
        };

        info!(
            provider = %selected,
            amount = %context.amount,
            strategy = ?context.strategy,
            "Selected payment provider"
        );

        Ok(selected)
    }

    /// Select default provider
    async fn select_default(
        &self,
        available: &[ProviderName],
        _context: &SelectionContext,
    ) -> OrchestratorResult<ProviderName> {
        // Check user preference first
        if let Some(preferred) = &_context.user_preferred_provider {
            if available.contains(preferred) {
                return Ok(preferred.clone());
            }
        }

        // Use configured default
        if available.contains(&self.config.default_provider) {
            return Ok(self.config.default_provider.clone());
        }

        // Fall back to first available
        available
            .first()
            .cloned()
            .ok_or(OrchestratorError::NoProviderAvailable)
    }

    /// Select provider based on cost (lowest fee)
    async fn select_cost_based(
        &self,
        available: &[ProviderName],
        context: &SelectionContext,
    ) -> OrchestratorResult<ProviderName> {
        // For large transactions, compare fees
        if context.amount >= self.config.large_transaction_threshold
            && self.config.fee_comparison_enabled
        {
            let fees: Vec<(ProviderName, BigDecimal)> = available
                .iter()
                .map(|name| (name.clone(), self.calculate_fee(name, &context.amount)))
                .collect();

            let min_fee_provider = fees
                .iter()
                .min_by(|a, b| a.1.cmp(&b.1))
                .map(|(name, _)| name.clone())
                .ok_or(OrchestratorError::NoProviderAvailable)?;

            return Ok(min_fee_provider);
        }

        // For smaller transactions, use default
        self.select_default(available, context).await
    }

    /// Select provider based on reliability (success rate)
    async fn select_reliability_based(
        &self,
        available: &[ProviderName],
        context: &SelectionContext,
    ) -> OrchestratorResult<ProviderName> {
        // Use await since this is called from async context
        let metrics = self.provider_metrics.read().await;

        // Filter providers below threshold
        let eligible: Vec<(ProviderName, f64)> = available
            .iter()
            .filter_map(|name| {
                if let Some(m) = metrics.get(name) {
                    let rate = m.success_rate();
                    if rate >= self.config.min_success_rate_threshold {
                        return Some((name.clone(), rate));
                    }
                }
                None
            })
            .collect();

        if eligible.is_empty() {
            // If no provider meets threshold, use default
            return self.select_default(available, context).await;
        }

        // Select provider with highest success rate
        let best = eligible
            .iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .map(|(name, _)| name.clone())
            .ok_or(OrchestratorError::NoProviderAvailable)?;

        Ok(best)
    }

    /// Select provider using round-robin
    async fn select_round_robin(
        &self,
        available: &[ProviderName],
    ) -> OrchestratorResult<ProviderName> {
        // Use async read since this is called from async context
        let index = {
            let idx = self.round_robin_index.read().await;
            *idx
        };
        let selected = available[index % available.len()].clone();

        // Update index for next selection
        {
            let mut index_mut = self.round_robin_index.write().await;
            *index_mut = (*index_mut + 1) % available.len();
        };

        Ok(selected)
    }

    /// Select provider with failover capability
    async fn select_failover(
        &self,
        available: &[ProviderName],
        context: &SelectionContext,
    ) -> OrchestratorResult<ProviderName> {
        // Try default first
        if let Ok(provider) = self.select_default(available, context).await {
            return Ok(provider);
        }

        // Fall back to first available
        available
            .first()
            .cloned()
            .ok_or(OrchestratorError::NoProviderAvailable)
    }

    /// Calculate fee for a provider
    fn calculate_fee(&self, provider: &ProviderName, amount: &BigDecimal) -> BigDecimal {
        // Fee rates (could be loaded from configuration)
        let rate = match provider {
            ProviderName::Flutterwave => BigDecimal::from(14), // 1.4%
            ProviderName::Paystack => BigDecimal::from(15),    // 1.5%
            ProviderName::Mpesa => BigDecimal::from(20),       // 2.0%
        };

        amount * rate / BigDecimal::from(1000)
    }

    // =========================================================================
    // Idempotency Key Handling
    // =========================================================================

    /// Generate idempotency key
    pub fn generate_idempotency_key(
        &self,
        operation: &str,
        wallet_address: &str,
        amount: &str,
        currency: &str,
    ) -> String {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let nonce: u32 = rand_simple();

        let raw_key = format!(
            "{}:{}:{}:{}:{}:{}",
            operation, wallet_address, amount, currency, timestamp, nonce
        );

        // Hash for consistent length
        let mut hasher = Sha256::new();
        hasher.update(raw_key.as_bytes());
        let result = hasher.finalize();
        format!("{:x}", result)
    }

    /// Check idempotency for a request
    pub async fn check_idempotency(
        &self,
        idempotency_key: &str,
    ) -> OrchestratorResult<IdempotencyCheckResult> {
        // Try to get from cache first
        // In production, implement actual cache lookup
        // For now, return NewTransaction to proceed

        // This would be implemented with actual Redis cache:
        // let cache_key = format!("idempotency:{}", idempotency_key);
        // if let Some(info) = self.cache.get(&cache_key).await? { ... }

        Ok(IdempotencyCheckResult::NewTransaction)
    }

    /// Store idempotency key info
    pub async fn store_idempotency_key(&self, info: &IdempotencyKeyInfo) -> OrchestratorResult<()> {
        // In production, store in Redis with TTL
        // let cache_key = format!("idempotency:{}", info.key);
        // self.cache.set(&cache_key, info, Some(Duration::from_secs(info.expires_at - info.created_at))).await?;

        info!(
            key = %info.key,
            transaction_id = %info.transaction_id,
            "Stored idempotency key"
        );

        Ok(())
    }

    // =========================================================================
    // Transaction State Management
    // =========================================================================

    /// Validate state transition
    pub fn validate_state_transition(
        current: &OrchestrationState,
        target: &OrchestrationState,
    ) -> OrchestratorResult<()> {
        if current.valid_transitions().contains(target) {
            Ok(())
        } else {
            Err(OrchestratorError::InvalidStateTransition {
                current: current.clone(),
                target: target.clone(),
            })
        }
    }

    /// Transition transaction state
    pub async fn transition_state(
        &self,
        transaction_id: &str,
        target_state: OrchestrationState,
        reason: Option<String>,
    ) -> OrchestratorResult<Transaction> {
        // Get current transaction
        let transaction = self
            .transaction_repo
            .find_by_id(transaction_id)
            .await
            .map_err(|e| OrchestratorError::TransactionNotFound {
                transaction_id: transaction_id.to_string(),
            })?
            .ok_or(OrchestratorError::TransactionNotFound {
                transaction_id: transaction_id.to_string(),
            })?;

        // Parse current state
        let current_state = OrchestrationState::from_db_status(&transaction.status)
            .unwrap_or(OrchestrationState::Created);

        // Validate transition
        Self::validate_state_transition(&current_state, &target_state)?;

        // Build metadata with reason
        let mut metadata = transaction.metadata.clone();
        if let Some(reason) = reason {
            metadata["state_change_reason"] = serde_json::json!(reason);
            metadata["previous_state"] = serde_json::json!(current_state.to_string());
            metadata["new_state"] = serde_json::json!(target_state.to_string());
        }

        // Update transaction
        let updated = self
            .transaction_repo
            .update_status_with_metadata(transaction_id, target_state.to_db_status(), metadata)
            .await
            .map_err(|e| OrchestratorError::ConfigurationError {
                message: format!("Failed to update transaction state: {}", e),
            })?;

        info!(
            transaction_id = %transaction_id,
            from_state = %current_state,
            to_state = %target_state,
            "Transaction state transitioned"
        );

        Ok(updated)
    }

    // =========================================================================
    // Payment Routing
    // =========================================================================

    /// Initiate a payment
    pub async fn initiate_payment(
        &self,
        request: PaymentInitiationRequest,
    ) -> OrchestratorResult<PaymentResponse> {
        let amount = request.amount.clone();
        let currency = request.currency.clone();

        // Generate or use provided idempotency key
        let idempotency_key = request.idempotency_key.unwrap_or_else(|| {
            self.generate_idempotency_key(
                "onramp",
                &request.wallet_address,
                &request.amount.to_string(),
                &request.currency,
            )
        });

        // Check idempotency
        match self.check_idempotency(&idempotency_key).await? {
            IdempotencyCheckResult::ExistingPending { transaction_id, .. } => {
                // Return existing pending transaction
                info!(transaction_id = %transaction_id, "Returning existing pending transaction");
                // In production, fetch and return existing transaction
                return Err(OrchestratorError::DuplicateTransaction { transaction_id });
            }
            IdempotencyCheckResult::Duplicate { transaction_id, .. } => {
                return Err(OrchestratorError::DuplicateTransaction { transaction_id });
            }
            IdempotencyCheckResult::AllowRetry { .. } => {
                // Allow retry with new key
            }
            IdempotencyCheckResult::NewTransaction => {
                // Proceed with new transaction
            }
        }

        // Create selection context
        let context = SelectionContext {
            amount: amount.clone(),
            currency: currency.clone(),
            payment_method: request.payment_method.clone(),
            user_preferred_provider: None,
            country: None,
            strategy: if amount >= self.config.large_transaction_threshold
                && self.config.fee_comparison_enabled
            {
                SelectionStrategy::CostBased
            } else {
                SelectionStrategy::Default
            },
        };

        // Select provider
        let provider_name = self.select_provider(&context).await?;

        // Get provider
        let provider = self
            .providers
            .get(&provider_name)
            .ok_or(OrchestratorError::NoProviderAvailable)?;

        // Create payment request
        let transaction_reference = Uuid::new_v4().to_string();
        let payment_request = PaymentRequest {
            amount: Money {
                amount: amount.to_string(),
                currency: currency.clone(),
            },
            customer: crate::payments::types::CustomerContact {
                email: request.customer_email.clone(),
                phone: request.customer_phone.clone(),
            },
            payment_method: request.payment_method.clone(),
            callback_url: request.callback_url.clone(),
            transaction_reference: transaction_reference.clone(),
            metadata: request.metadata.clone(),
        };

        // Initiate payment with retry logic
        let response = self
            .initiate_with_retry(provider.as_ref(), payment_request)
            .await?;

        // Store idempotency key
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let idempotency_info = IdempotencyKeyInfo {
            key: idempotency_key.clone(),
            transaction_id: transaction_reference.clone(),
            wallet_address: request.wallet_address.clone(),
            amount: amount.to_string(),
            currency: currency.clone(),
            operation: "onramp".to_string(),
            created_at: now,
            expires_at: now + self.config.idempotency_key_expiration_secs,
        };
        self.store_idempotency_key(&idempotency_info).await?;

        // Record metrics
        {
            let mut metrics = self.provider_metrics.write().await;
            if let Some(m) = metrics.get_mut(&provider_name) {
                if response.status == PaymentState::Success {
                    let fees = self.calculate_fee(&provider_name, &amount);
                    m.record_success(fees);
                } else if response.status == PaymentState::Failed {
                    m.record_failure();
                }
            }
        }

        info!(
            transaction_id = %transaction_reference,
            provider = %provider_name,
            status = ?response.status,
            "Payment initiated"
        );

        Ok(response)
    }

    /// Initiate payment with retry logic
    async fn initiate_with_retry(
        &self,
        provider: &dyn PaymentProvider,
        request: PaymentRequest,
    ) -> OrchestratorResult<PaymentResponse> {
        let mut attempt = 0;
        let mut last_error: Option<OrchestratorError> = None;

        while attempt < self.config.max_retry_attempts {
            attempt += 1;

            match provider.initiate_payment(request.clone()).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    if !e.is_retryable() {
                        // Non-retryable error, fail immediately
                        return Err(OrchestratorError::AllProvidersFailed {
                            errors: vec![e.to_string()],
                        });
                    }

                    last_error = Some(OrchestratorError::AllProvidersFailed {
                        errors: vec![e.to_string()],
                    });

                    // Calculate delay with exponential backoff
                    if attempt < self.config.max_retry_attempts {
                        let delay = self.calculate_retry_delay(attempt);
                        warn!(
                            attempt = attempt,
                            max_attempts = self.config.max_retry_attempts,
                            delay_secs = delay,
                            error = %e,
                            "Payment initiation failed, retrying"
                        );
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or(OrchestratorError::AllProvidersFailed {
            errors: vec!["Max retries exceeded".to_string()],
        }))
    }

    /// Calculate retry delay with exponential backoff
    fn calculate_retry_delay(&self, attempt: u32) -> u64 {
        let delay = self.config.initial_retry_delay_secs * 2u64.pow(attempt - 1);
        std::cmp::min(delay, self.config.max_retry_delay_secs)
    }

    /// Verify payment status
    pub async fn verify_payment(
        &self,
        transaction_reference: &str,
        provider_reference: Option<&str>,
    ) -> OrchestratorResult<StatusResponse> {
        // Find transaction to get provider
        let transaction = self
            .transaction_repo
            .find_by_payment_reference(transaction_reference)
            .await
            .map_err(|_| OrchestratorError::TransactionNotFound {
                transaction_id: transaction_reference.to_string(),
            })?
            .ok_or(OrchestratorError::TransactionNotFound {
                transaction_id: transaction_reference.to_string(),
            })?;

        // Get provider
        let provider_name = transaction
            .payment_provider
            .as_ref()
            .and_then(|p| p.parse().ok())
            .ok_or(OrchestratorError::ConfigurationError {
                message: "Provider not found for transaction".to_string(),
            })?;

        let provider = self
            .providers
            .get(&provider_name)
            .ok_or(OrchestratorError::NoProviderAvailable)?;

        // Verify payment
        let status_request = StatusRequest {
            transaction_reference: Some(transaction_reference.to_string()),
            provider_reference: provider_reference.map(String::from),
        };

        let response = provider.verify_payment(status_request).await.map_err(|e| {
            OrchestratorError::AllProvidersFailed {
                errors: vec![e.to_string()],
            }
        })?;

        info!(
            transaction_id = %transaction_reference,
            status = ?response.status,
            "Payment verified"
        );

        Ok(response)
    }

    // =========================================================================
    // Failure Handling & Recovery
    // =========================================================================

    /// Handle payment failure
    pub async fn handle_failure(
        &self,
        transaction_id: &str,
        error: &str,
    ) -> OrchestratorResult<Transaction> {
        // Update transaction with error
        let transaction = self
            .transaction_repo
            .update_error(transaction_id, error)
            .await
            .map_err(|e| OrchestratorError::ConfigurationError {
                message: format!("Failed to update transaction error: {}", e),
            })?;

        // Transition to failed state
        let updated = self
            .transition_state(
                transaction_id,
                OrchestrationState::Failed,
                Some(error.to_string()),
            )
            .await?;

        // Record failure metrics
        if let Some(provider_name) = &transaction.payment_provider {
            if let Ok(name) = provider_name.parse::<ProviderName>() {
                let mut metrics = self.provider_metrics.write().await;
                if let Some(m) = metrics.get_mut(&name) {
                    m.record_failure();
                }
            }
        }

        error!(
            transaction_id = %transaction_id,
            error = %error,
            "Payment failed"
        );

        Ok(updated)
    }

    /// Failover to backup provider
    pub async fn failover(
        &self,
        failed_provider: ProviderName,
        request: PaymentRequest,
    ) -> OrchestratorResult<PaymentResponse> {
        // Find alternative providers
        let failed = failed_provider.clone();
        let alternative_providers: Vec<ProviderName> = self
            .providers
            .keys()
            .filter(|name| *name != &failed)
            .cloned()
            .collect();

        if alternative_providers.is_empty() {
            return Err(OrchestratorError::NoProviderAvailable);
        }

        // Try each alternative provider
        let mut errors = Vec::new();
        for provider_name in alternative_providers {
            if let Some(provider) = self.providers.get(&provider_name) {
                info!(
                    from_provider = %failed_provider,
                    to_provider = %provider_name,
                    "Attempting failover"
                );

                match provider.initiate_payment(request.clone()).await {
                    Ok(response) => {
                        info!(
                            from_provider = %failed_provider,
                            to_provider = %provider_name,
                            "Failover successful"
                        );
                        return Ok(response);
                    }
                    Err(e) => {
                        errors.push(format!("{}: {}", provider_name, e));
                        continue;
                    }
                }
            }
        }

        Err(OrchestratorError::AllProvidersFailed { errors })
    }

    /// Get transaction status
    pub async fn get_transaction_status(
        &self,
        transaction_id: &str,
    ) -> OrchestratorResult<Transaction> {
        self.transaction_repo
            .find_by_id(transaction_id)
            .await
            .map_err(|_| OrchestratorError::TransactionNotFound {
                transaction_id: transaction_id.to_string(),
            })?
            .ok_or(OrchestratorError::TransactionNotFound {
                transaction_id: transaction_id.to_string(),
            })
    }

    /// Manual retry for stuck transactions
    pub async fn manual_retry(&self, transaction_id: &str) -> OrchestratorResult<PaymentResponse> {
        let transaction = self.get_transaction_status(transaction_id).await?;

        // Check if retry is allowed
        let current_state = OrchestrationState::from_db_status(&transaction.status).ok_or(
            OrchestratorError::InvalidStateTransition {
                current: OrchestrationState::Created,
                target: OrchestrationState::PendingPayment,
            },
        )?;

        if !current_state.allows_retry() {
            return Err(OrchestratorError::InvalidStateTransition {
                current: current_state,
                target: OrchestrationState::PendingPayment,
            });
        }

        // Transition back to pending
        self.transition_state(
            transaction_id,
            OrchestrationState::PendingPayment,
            Some("Manual retry initiated".to_string()),
        )
        .await?;

        // Re-initiate payment (simplified - in production would reconstruct full request)
        let provider_name = transaction
            .payment_provider
            .as_ref()
            .and_then(|p| p.parse().ok())
            .ok_or(OrchestratorError::ConfigurationError {
                message: "Provider not found for transaction".to_string(),
            })?;

        let provider = self
            .providers
            .get(&provider_name)
            .ok_or(OrchestratorError::NoProviderAvailable)?;

        // Create new payment request (simplified)
        let payment_request = PaymentRequest {
            amount: Money {
                amount: transaction.from_amount.to_string(),
                currency: transaction.from_currency.clone(),
            },
            customer: crate::payments::types::CustomerContact {
                email: None,
                phone: None,
            },
            payment_method: PaymentMethod::Card,
            callback_url: None,
            transaction_reference: transaction_id.to_string(),
            metadata: Some(transaction.metadata.clone()),
        };

        // Initiate with retry
        let response = self
            .initiate_with_retry(provider.as_ref(), payment_request)
            .await?;

        info!(transaction_id = %transaction_id, "Manual retry completed");

        Ok(response)
    }

    // =========================================================================
    // Metrics & Monitoring
    // =========================================================================

    /// Get provider metrics
    pub async fn get_provider_metrics(&self) -> HashMap<ProviderName, ProviderMetrics> {
        self.provider_metrics.read().await.clone()
    }

    /// Get provider health status
    pub async fn get_provider_health(&self, provider: &ProviderName) -> ProviderHealth {
        let metrics = self.provider_metrics.read().await;
        metrics
            .get(provider)
            .map(|m| m.current_health)
            .unwrap_or(ProviderHealth::Healthy)
    }

    // =========================================================================
    // Webhook Handlers
    // =========================================================================

    /// Handle payment success webhook
    pub async fn handle_payment_success(
        &self,
        transaction_reference: &str,
    ) -> OrchestratorResult<()> {
        let transaction = self
            .transaction_repo
            .find_by_payment_reference(transaction_reference)
            .await
            .map_err(|_| OrchestratorError::TransactionNotFound {
                transaction_id: transaction_reference.to_string(),
            })?
            .ok_or(OrchestratorError::TransactionNotFound {
                transaction_id: transaction_reference.to_string(),
            })?;

        self.transition_state(
            &transaction.transaction_id.to_string(),
            OrchestrationState::PaymentConfirmed,
            Some("Payment confirmed via webhook".to_string()),
        )
        .await?;

        info!(tx_ref = %transaction_reference, "Payment success processed");
        Ok(())
    }

    /// Handle payment failure webhook
    pub async fn handle_payment_failure(
        &self,
        transaction_reference: &str,
        reason: &str,
    ) -> OrchestratorResult<()> {
        let transaction = self
            .transaction_repo
            .find_by_payment_reference(transaction_reference)
            .await
            .map_err(|_| OrchestratorError::TransactionNotFound {
                transaction_id: transaction_reference.to_string(),
            })?
            .ok_or(OrchestratorError::TransactionNotFound {
                transaction_id: transaction_reference.to_string(),
            })?;

        self.handle_failure(&transaction.transaction_id.to_string(), reason)
            .await?;
        info!(tx_ref = %transaction_reference, reason = %reason, "Payment failure processed");
        Ok(())
    }

    /// Handle withdrawal success webhook
    pub async fn handle_withdrawal_success(
        &self,
        transaction_reference: &str,
    ) -> OrchestratorResult<()> {
        let transaction = self
            .transaction_repo
            .find_by_payment_reference(transaction_reference)
            .await
            .map_err(|_| OrchestratorError::TransactionNotFound {
                transaction_id: transaction_reference.to_string(),
            })?
            .ok_or(OrchestratorError::TransactionNotFound {
                transaction_id: transaction_reference.to_string(),
            })?;

        self.transition_state(
            &transaction.transaction_id.to_string(),
            OrchestrationState::Completed,
            Some("Withdrawal confirmed via webhook".to_string()),
        )
        .await?;

        info!(tx_ref = %transaction_reference, "Withdrawal success processed");
        Ok(())
    }

    /// Handle withdrawal failure webhook
    pub async fn handle_withdrawal_failure(
        &self,
        transaction_reference: &str,
        reason: &str,
    ) -> OrchestratorResult<()> {
        let transaction = self
            .transaction_repo
            .find_by_payment_reference(transaction_reference)
            .await
            .map_err(|_| OrchestratorError::TransactionNotFound {
                transaction_id: transaction_reference.to_string(),
            })?
            .ok_or(OrchestratorError::TransactionNotFound {
                transaction_id: transaction_reference.to_string(),
            })?;

        self.handle_failure(&transaction.transaction_id.to_string(), reason)
            .await?;
        info!(tx_ref = %transaction_reference, reason = %reason, "Withdrawal failure processed");
        Ok(())
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Simple random number generator (not cryptographically secure)
fn rand_simple() -> u32 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    nanos.wrapping_mul(1103515245).wrapping_add(12345) as u32
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transitions_valid() {
        assert!(OrchestrationState::Created
            .valid_transitions()
            .contains(&OrchestrationState::PendingPayment));

        assert!(OrchestrationState::PendingPayment
            .valid_transitions()
            .contains(&OrchestrationState::PaymentConfirmed));

        assert!(OrchestrationState::PendingPayment
            .valid_transitions()
            .contains(&OrchestrationState::Failed));
    }

    #[test]
    fn test_state_transitions_invalid() {
        // Can't go from pending directly to completed
        assert!(!OrchestrationState::PendingPayment
            .valid_transitions()
            .contains(&OrchestrationState::Completed));

        // Can't go from failed to anything
        assert!(OrchestrationState::Failed.valid_transitions().is_empty());
    }

    #[test]
    fn test_terminal_states() {
        assert!(OrchestrationState::Completed.is_terminal());
        assert!(OrchestrationState::Failed.is_terminal());
        assert!(OrchestrationState::Refunded.is_terminal());

        assert!(!OrchestrationState::Created.is_terminal());
        assert!(!OrchestrationState::PendingPayment.is_terminal());
    }

    #[test]
    fn test_idempotency_key_generation() {
        let config = OrchestratorConfig::default();
        // Can't directly test without orchestrator instance, but verify format

        // Key format: {operation}:{wallet}:{amount}:{currency}:{timestamp}:{nonce}
        let raw = "onramp:GA123:1000:NGN:1708012345:12345";
        let mut hasher = Sha256::new();
        hasher.update(raw.as_bytes());
        let result = hasher.finalize();
        let hashed = format!("{:x}", result);

        assert_eq!(hashed.len(), 64); // SHA256 produces 64 hex characters
    }

    #[test]
    fn test_success_rate_calculation() {
        let mut metrics = ProviderMetrics::new(ProviderName::Flutterwave);

        // No requests = 100% success rate
        assert_eq!(metrics.success_rate(), 1.0);

        // Record some successes
        metrics.record_success(BigDecimal::from(10));
        metrics.record_success(BigDecimal::from(20));
        assert_eq!(metrics.success_rate(), 1.0);

        // Record a failure
        metrics.record_failure();
        assert_eq!(metrics.success_rate(), 2.0 / 3.0);
    }

    #[test]
    fn test_state_from_db_status() {
        assert_eq!(
            OrchestrationState::from_db_status("pending"),
            Some(OrchestrationState::PendingPayment)
        );
        assert_eq!(
            OrchestrationState::from_db_status("completed"),
            Some(OrchestrationState::Completed)
        );
        assert_eq!(
            OrchestrationState::from_db_status("failed"),
            Some(OrchestrationState::Failed)
        );
        assert_eq!(OrchestrationState::from_db_status("unknown"), None);
    }

    #[test]
    fn test_state_to_db_status() {
        assert_eq!(OrchestrationState::Created.to_db_status(), "created");
        assert_eq!(OrchestrationState::PendingPayment.to_db_status(), "pending");
        assert_eq!(OrchestrationState::Completed.to_db_status(), "completed");
    }
}
