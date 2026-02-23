use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// ChainType enumeration for blockchain dispatching
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ChainType {
    /// Stellar blockchain (CNGN stablecoin primary chain)
    Stellar,
    /// Ethereum/EVM blockchain
    Ethereum,
    /// Bitcoin blockchain
    Bitcoin,
}

impl ChainType {
    /// Get chain identifier string
    pub fn as_str(&self) -> &'static str {
        match self {
            ChainType::Stellar => "stellar",
            ChainType::Ethereum => "ethereum",
            ChainType::Bitcoin => "bitcoin",
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "stellar" => Some(ChainType::Stellar),
            "ethereum" | "evm" => Some(ChainType::Ethereum),
            "bitcoin" | "btc" => Some(ChainType::Bitcoin),
            _ => None,
        }
    }
}

impl std::fmt::Display for ChainType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Transaction parameters for building transactions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxParams {
    /// Recipient address
    pub to: String,
    /// Asset/code to send (e.g., "cNGN", "XLM", "ETH")
    pub asset_code: String,
    /// Asset issuer (None for native assets)
    pub issuer: Option<String>,
    /// Amount to send (as string to avoid float precision issues)
    pub amount: String,
    /// Source/sender address
    pub from: Option<String>,
    /// Memo/note for the transaction (optional)
    pub memo: Option<String>,
}

/// Fee estimation for transactions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeEstimate {
    /// Estimated fee amount
    pub fee: String,
    /// Fee unit (e.g., "stroops", "gas", "sats")
    pub fee_unit: String,
    /// Estimated confirmation time in seconds
    pub estimated_confirmation_time_secs: u64,
    /// Whether this is a rough estimate
    pub is_estimate: bool,
}

/// Aggregated balance result with converted values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedBalance {
    /// Chain identifier
    pub chain: String,
    /// Asset code
    pub asset_code: String,
    /// Original balance
    pub balance: String,
    /// Balance in USD equivalent (if conversion available)
    pub usd_equivalent: Option<String>,
    /// Conversion rate used
    pub conversion_rate: Option<String>,
}

/// Total aggregated balance across all chains
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotalBalance {
    /// Total balance in each currency
    pub balances: Vec<AggregatedBalance>,
    /// Total USD equivalent
    pub total_usd: Option<String>,
    /// Timestamp of calculation
    pub calculated_at: String,
}

/// Common result type for blockchain operations
pub type BlockchainResult<T> = Result<T, BlockchainError>;

/// Unified error type for all blockchain operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BlockchainError {
    /// Account not found on the blockchain
    AccountNotFound { address: String },
    /// Invalid address format
    InvalidAddress { address: String },
    /// Network communication error
    NetworkError { message: String },
    /// Transaction submission failed
    TransactionFailed { message: String },
    /// Operation timed out
    Timeout { seconds: u64 },
    /// Rate limit exceeded
    RateLimitExceeded,
    /// Insufficient balance for operation
    InsufficientBalance { required: String, available: String },
    /// Asset not found or not trusted
    AssetNotFound { asset_code: String },
    /// Configuration error
    ConfigError { message: String },
    /// Serialization/deserialization error
    SerializationError { message: String },
    /// Generic error
    Other { message: String },
}

impl std::fmt::Display for BlockchainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockchainError::AccountNotFound { address } => {
                write!(f, "Account not found: {}", address)
            }
            BlockchainError::InvalidAddress { address } => {
                write!(f, "Invalid address: {}", address)
            }
            BlockchainError::NetworkError { message } => write!(f, "Network error: {}", message),
            BlockchainError::TransactionFailed { message } => {
                write!(f, "Transaction failed: {}", message)
            }
            BlockchainError::Timeout { seconds } => {
                write!(f, "Operation timed out after {} seconds", seconds)
            }
            BlockchainError::RateLimitExceeded => write!(f, "Rate limit exceeded"),
            BlockchainError::InsufficientBalance {
                required,
                available,
            } => {
                write!(
                    f,
                    "Insufficient balance: required {}, available {}",
                    required, available
                )
            }
            BlockchainError::AssetNotFound { asset_code } => {
                write!(f, "Asset not found: {}", asset_code)
            }
            BlockchainError::ConfigError { message } => write!(f, "Config error: {}", message),
            BlockchainError::SerializationError { message } => {
                write!(f, "Serialization error: {}", message)
            }
            BlockchainError::Other { message } => write!(f, "{}", message),
        }
    }
}

impl std::error::Error for BlockchainError {}

/// Represents a balance for a specific asset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetBalance {
    /// Asset code (e.g., "XLM", "cNGN", "AFRI")
    pub asset_code: String,
    /// Asset issuer address (None for native assets)
    pub issuer: Option<String>,
    /// Balance amount as string
    pub balance: String,
    /// Asset type (e.g., "native", "credit_alphanum4")
    pub asset_type: String,
    /// Optional limit for trustline
    pub limit: Option<String>,
}

/// Represents account information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    /// Account address
    pub address: String,
    /// Account sequence number (for transaction ordering)
    pub sequence: String,
    /// List of asset balances
    pub balances: Vec<AssetBalance>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Health status for blockchain connection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainHealthStatus {
    /// Whether the chain is healthy
    pub is_healthy: bool,
    /// Chain identifier (e.g., "stellar", "ethereum")
    pub chain_id: String,
    /// Response time in milliseconds
    pub response_time_ms: u64,
    /// Last check timestamp
    pub last_check: String,
    /// Error message if unhealthy
    pub error_message: Option<String>,
}

/// Transaction submission result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionResult {
    /// Transaction hash
    pub hash: String,
    /// Whether transaction was successful
    pub successful: bool,
    /// Ledger number where transaction was included
    pub ledger: Option<i64>,
    /// Transaction fee charged
    pub fee_charged: Option<String>,
    /// Raw response from blockchain
    pub raw_response: serde_json::Value,
}

/// Unified blockchain service trait
///
/// This trait provides a common interface for interacting with different blockchains.
/// Implementations should handle chain-specific details internally.
#[async_trait]
pub trait BlockchainService: Send + Sync {
    /// Get the chain identifier (e.g., "stellar", "ethereum")
    fn chain_id(&self) -> &str;

    /// Check if an account exists on the blockchain
    async fn account_exists(&self, address: &str) -> BlockchainResult<bool>;

    /// Get account information including balances
    async fn get_account(&self, address: &str) -> BlockchainResult<AccountInfo>;

    /// Get all balances for an account
    async fn get_balances(&self, address: &str) -> BlockchainResult<Vec<AssetBalance>>;

    /// Get balance for a specific asset
    async fn get_asset_balance(
        &self,
        address: &str,
        asset_code: &str,
        issuer: Option<&str>,
    ) -> BlockchainResult<Option<String>>;

    /// Submit a signed transaction to the blockchain
    async fn submit_transaction(&self, signed_tx: &str) -> BlockchainResult<TransactionResult>;

    /// Get transaction details by hash
    async fn get_transaction(&self, tx_hash: &str) -> BlockchainResult<TransactionResult>;

    /// Perform health check on blockchain connection
    async fn health_check(&self) -> BlockchainResult<ChainHealthStatus>;

    /// Validate an address format
    fn validate_address(&self, address: &str) -> BlockchainResult<()>;

    /// Estimate transaction fee
    async fn estimate_fee(&self, params: &TxParams) -> BlockchainResult<FeeEstimate>;

    /// Build a transaction (returns chain-specific transaction envelope)
    async fn build_transaction(
        &self,
        params: TxParams,
        source_address: &str,
    ) -> BlockchainResult<String>;
}

/// Multi-chain balance aggregator
///
/// Aggregates balances across multiple blockchain services
pub struct MultiChainBalanceAggregator {
    chains: Vec<Arc<dyn BlockchainService>>,
}

impl MultiChainBalanceAggregator {
    /// Create a new aggregator with the given blockchain services
    pub fn new(chains: Vec<Arc<dyn BlockchainService>>) -> Self {
        Self { chains }
    }

    /// Get balances across all chains for an address (parallel execution)
    pub async fn get_all_balances(
        &self,
        address: &str,
    ) -> HashMap<String, BlockchainResult<Vec<AssetBalance>>> {
        use futures::stream::{self, StreamExt};

        let chains: Vec<_> = self.chains.iter().collect();
        let address = address.to_string();

        let results: Vec<(String, BlockchainResult<Vec<AssetBalance>>)> = stream::iter(chains)
            .map(|chain| {
                let chain_id = chain.chain_id().to_string();
                let addr = address.clone();
                async move {
                    let balances = chain.get_balances(&addr).await;
                    (chain_id, balances)
                }
            })
            .buffer_unordered(3) // Process up to 3 chains concurrently
            .collect()
            .await;

        results.into_iter().collect()
    }

    /// Get total balance for a specific asset across all chains (parallel)
    pub async fn get_asset_total(
        &self,
        address: &str,
        asset_code: &str,
    ) -> BlockchainResult<String> {
        use futures::stream::{self, StreamExt};

        let chains: Vec<_> = self.chains.iter().collect();
        let address = address.to_string();
        let asset = asset_code.to_string();

        let results: Vec<BlockchainResult<Option<String>>> = stream::iter(chains)
            .map(|chain| {
                let addr = address.clone();
                let asset = asset.clone();
                async move { chain.get_asset_balance(&addr, &asset, None).await }
            })
            .buffer_unordered(3)
            .collect()
            .await;

        let mut total = 0.0f64;
        for result in results {
            if let Ok(Some(balance)) = result {
                if let Ok(amount) = balance.parse::<f64>() {
                    total += amount;
                }
            }
        }

        Ok(total.to_string())
    }

    /// Aggregate balances with currency conversion (parallel)
    #[allow(clippy::type_complexity)]
    pub async fn aggregate_balances_with_conversion<F>(
        &self,
        address: &str,
        converter: F,
    ) -> BlockchainResult<TotalBalance>
    where
        F: for<'a> Fn(&'a str, &'a str) -> futures::future::BoxFuture<'a, BlockchainResult<String>>
            + Send
            + Sync,
    {
        use futures::stream::{self, StreamExt};

        let chains: Vec<_> = self.chains.iter().collect();
        let address = address.to_string();

        let results: Vec<BlockchainResult<Vec<AggregatedBalance>>> = stream::iter(chains)
            .map(|chain| {
                let chain_id = chain.chain_id().to_string();
                let address = address.clone();
                let converter = &converter;
                async move {
                    let balances = chain.get_balances(&address).await?;
                    let mut aggregated = Vec::new();

                    for balance in balances {
                        let usd = converter(&balance.asset_code, "USD").await.ok();
                        let rate = usd.as_deref();

                        let usd_equivalent =
                            if let (Some(rate), Ok(amt)) = (rate, balance.balance.parse::<f64>()) {
                                if let Ok(r) = rate.parse::<f64>() {
                                    Some((amt * r).to_string())
                                } else {
                                    None
                                }
                            } else {
                                None
                            };

                        aggregated.push(AggregatedBalance {
                            chain: chain_id.clone(),
                            asset_code: balance.asset_code.clone(),
                            balance: balance.balance,
                            usd_equivalent,
                            conversion_rate: rate.map(|s| s.to_string()),
                        });
                    }

                    Ok::<_, BlockchainError>(aggregated)
                }
            })
            .buffer_unordered(3)
            .collect()
            .await;

        let mut all_balances: Vec<AggregatedBalance> = Vec::new();
        let mut total_usd = 0.0f64;

        for result in results {
            all_balances.extend(result?);
        }

        for balance in &all_balances {
            if let Some(ref usd) = balance.usd_equivalent {
                if let Ok(amt) = usd.parse::<f64>() {
                    total_usd += amt;
                }
            }
        }

        Ok(TotalBalance {
            balances: all_balances,
            total_usd: if total_usd > 0.0 {
                Some(total_usd.to_string())
            } else {
                None
            },
            calculated_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    /// Check health of all chains (parallel)
    pub async fn health_check_all(&self) -> HashMap<String, ChainHealthStatus> {
        use futures::stream::{self, StreamExt};

        let chains: Vec<_> = self.chains.iter().collect();

        stream::iter(chains)
            .map(|chain| {
                let chain_id = chain.chain_id().to_string();
                async move {
                    let status = chain.health_check().await;
                    (chain_id, status)
                }
            })
            .buffer_unordered(3)
            .filter_map(|r| async move {
                match r {
                    (id, Ok(status)) => Some((id, status)),
                    _ => None,
                }
            })
            .collect()
            .await
    }
}

/// Transaction handler for chain-agnostic transaction operations
pub struct TransactionHandler {
    service: Arc<dyn BlockchainService>,
}

impl TransactionHandler {
    /// Create a new transaction handler with the given service
    pub fn new(service: Arc<dyn BlockchainService>) -> Self {
        Self { service }
    }

    /// Build and validate a transaction
    pub async fn build_transaction(
        &self,
        params: TxParams,
    ) -> BlockchainResult<TransactionBuilder> {
        // Validate recipient address
        self.service.validate_address(&params.to)?;

        // Get source address if not provided
        let from = params.from.clone().ok_or_else(|| BlockchainError::Other {
            message: "Source address required".to_string(),
        })?;

        // Validate source address
        self.service.validate_address(&from)?;

        // Get fee estimate
        let fee = self.service.estimate_fee(&params).await?;

        Ok(TransactionBuilder {
            params,
            fee,
            chain_id: self.service.chain_id().to_string(),
        })
    }

    /// Submit a built transaction
    pub async fn submit_transaction(&self, signed_tx: &str) -> BlockchainResult<TransactionResult> {
        self.service.submit_transaction(signed_tx).await
    }

    /// Get transaction status
    pub async fn get_transaction_status(
        &self,
        tx_hash: &str,
    ) -> BlockchainResult<TransactionResult> {
        self.service.get_transaction(tx_hash).await
    }
}

/// Transaction builder for constructing transactions
#[derive(Debug, Clone)]
pub struct TransactionBuilder {
    params: TxParams,
    fee: FeeEstimate,
    chain_id: String,
}

impl TransactionBuilder {
    /// Get the transaction parameters
    pub fn params(&self) -> &TxParams {
        &self.params
    }

    /// Get the fee estimate
    pub fn fee(&self) -> &FeeEstimate {
        &self.fee
    }

    /// Get the chain ID
    pub fn chain_id(&self) -> &str {
        &self.chain_id
    }

    /// Convert to JSON for signing
    pub fn to_json(&self) -> BlockchainResult<String> {
        serde_json::to_string(&self.params).map_err(|e| BlockchainError::SerializationError {
            message: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_type_as_str() {
        assert_eq!(ChainType::Stellar.as_str(), "stellar");
        assert_eq!(ChainType::Ethereum.as_str(), "ethereum");
        assert_eq!(ChainType::Bitcoin.as_str(), "bitcoin");
    }

    #[test]
    fn test_chain_type_from_str() {
        assert_eq!(ChainType::from_str("stellar"), Some(ChainType::Stellar));
        assert_eq!(ChainType::from_str("STELLAR"), Some(ChainType::Stellar));
        assert_eq!(ChainType::from_str("ethereum"), Some(ChainType::Ethereum));
        assert_eq!(ChainType::from_str("evm"), Some(ChainType::Ethereum));
        assert_eq!(ChainType::from_str("bitcoin"), Some(ChainType::Bitcoin));
        assert_eq!(ChainType::from_str("btc"), Some(ChainType::Bitcoin));
        assert_eq!(ChainType::from_str("invalid"), None);
    }

    #[test]
    fn test_chain_type_display() {
        assert_eq!(format!("{}", ChainType::Stellar), "stellar");
        assert_eq!(format!("{}", ChainType::Ethereum), "ethereum");
        assert_eq!(format!("{}", ChainType::Bitcoin), "bitcoin");
    }

    #[test]
    fn test_tx_params_creation() {
        let params = TxParams {
            to: "GCJRI5CIWK5IU67Q6DGA7QW52JDKRO7JEAHQKFNDUJUPEZGURDBX3LDX".to_string(),
            asset_code: "cNGN".to_string(),
            issuer: Some("GAQJF5G7E4L7G5F3J5C5XQZ7VY5RW3J3D5C5X5Z7VY5RW3J3D5C5XQZ7V".to_string()),
            amount: "100.50".to_string(),
            from: Some("GAQJF5G7E4L7G5F3J5C5XQZ7VY5RW3J3D5C5X5Z7VY5RW3J3D5C5XQZ7V".to_string()),
            memo: Some("test".to_string()),
        };

        assert_eq!(params.asset_code, "cNGN");
        assert_eq!(params.amount, "100.50");
    }

    #[test]
    fn test_fee_estimate_creation() {
        let fee = FeeEstimate {
            fee: "500".to_string(),
            fee_unit: "stroops".to_string(),
            estimated_confirmation_time_secs: 5,
            is_estimate: true,
        };

        assert_eq!(fee.fee, "500");
        assert_eq!(fee.fee_unit, "stroops");
        assert!(fee.is_estimate);
    }

    #[test]
    fn test_aggregated_balance_creation() {
        let balance = AggregatedBalance {
            chain: "stellar".to_string(),
            asset_code: "cNGN".to_string(),
            balance: "1000".to_string(),
            usd_equivalent: Some("1.50".to_string()),
            conversion_rate: Some("0.0015".to_string()),
        };

        assert_eq!(balance.chain, "stellar");
        assert_eq!(balance.asset_code, "cNGN");
        assert_eq!(balance.balance, "1000");
        assert!(balance.usd_equivalent.is_some());
    }

    #[test]
    fn test_total_balance_creation() {
        let balances = vec![AggregatedBalance {
            chain: "stellar".to_string(),
            asset_code: "cNGN".to_string(),
            balance: "1000".to_string(),
            usd_equivalent: Some("1.50".to_string()),
            conversion_rate: Some("0.0015".to_string()),
        }];

        let total = TotalBalance {
            balances,
            total_usd: Some("1.50".to_string()),
            calculated_at: "2024-01-01T00:00:00Z".to_string(),
        };

        assert_eq!(total.balances.len(), 1);
        assert!(total.total_usd.is_some());
    }

    #[test]
    fn test_blockchain_error_display() {
        let err = BlockchainError::InvalidAddress {
            address: "invalid".to_string(),
        };
        assert!(err.to_string().contains("Invalid address"));

        let err = BlockchainError::AccountNotFound {
            address: "GCXXX".to_string(),
        };
        assert!(err.to_string().contains("Account not found"));

        let err = BlockchainError::NetworkError {
            message: "Connection failed".to_string(),
        };
        assert!(err.to_string().contains("Network error"));
    }
}
