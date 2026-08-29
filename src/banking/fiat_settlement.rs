//! Fiat-to-Stablecoin Execution Module
//! Automatically mints or transfers equivalent cNGN upon verified bank deposit

use crate::banking::integrations::{FiatSettlement, SettlementStatus};
// REMOVED: use crate::chains::ChainService;
use crate::wallet::WalletService;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info, instrument};
use uuid::Uuid;

/// Fiat-to-cNGN Configuration
#[derive(Debug, Clone)]
pub struct FiatSettlementConfig {
    /// Exchange rate (NGN to cNGN) - typically 1:1 or based on oracle
    pub exchange_rate: Decimal,
    /// Minimum settlement amount in NGN
    pub min_settlement_amount: Decimal,
    /// Maximum settlement amount in NGN
    pub max_settlement_amount: Decimal,
    /// Settlement confirmation required blocks
    pub confirmation_blocks: u32,
    /// Auto-mint enabled
    pub auto_mint_enabled: bool,
}

impl Default for FiatSettlementConfig {
    fn default() -> Self {
        Self {
            exchange_rate: Decimal::from(1),
            min_settlement_amount: Decimal::from(100),
            max_settlement_amount: Decimal::from(10_000_000),
            confirmation_blocks: 1,
            auto_mint_enabled: true,
        }
    }
}

/// Settlement Execution Result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementExecutionResult {
    pub settlement_id: Uuid,
    pub user_id: Uuid,
    pub fiat_amount: Decimal,
    pub cngn_amount: Decimal,
    pub wallet_address: String,
    pub transaction_hash: Option<String>,
    pub status: SettlementStatus,
    pub executed_at: chrono::DateTime<Utc>,
}

/// Fiat-to-Stablecoin Execution Service
pub struct FiatSettlementExecutor {
    config: FiatSettlementConfig,
    wallet_service: Arc<WalletService>,
    chain_service: Option<Arc<ChainService>>,
}

impl FiatSettlementExecutor {
    pub fn new(
        config: FiatSettlementConfig,
        wallet_service: Arc<WalletService>,
        chain_service: Option<Arc<ChainService>>,
    ) -> Self {
        Self {
            config,
            wallet_service,
            chain_service,
        }
    }

    /// Execute fiat settlement - mint cNGN for verified deposit
    #[instrument(skip(self, settlement), fields(settlement_id = %settlement.id, user_id = %settlement.user_id))]
    pub async fn execute_settlement(
        &self,
        settlement: &mut FiatSettlement,
    ) -> Result<SettlementExecutionResult, SettlementError> {
        info!(
            amount = %settlement.amount,
            "Executing fiat settlement"
        );

        // 1. Validate settlement amount
        self.validate_amount(settlement.amount)?;

        // 2. Get user's wallet address
        let wallet_address = self.get_user_wallet(settlement.user_id).await?;
        settlement.wallet_address = Some(wallet_address.clone());

        // 3. Calculate cNGN amount
        let cngn_amount = self.calculate_cngn_amount(settlement.amount);
        settlement.cngn_amount = Some(cngn_amount);

        // 4. Update status to minting
        settlement.settlement_status = SettlementStatus::Minting;
        settlement.minted_at = Some(Utc::now());

        // 5. Execute mint
        let transaction_hash = self.mint_cngn(&wallet_address, cngn_amount).await?;

        // 6. Update settlement record
        settlement.cngn_minted = true;
        settlement.settlement_status = SettlementStatus::Completed;
        settlement.completed_at = Some(Utc::now());

        info!(
            settlement_id = %settlement.id,
            cngn_amount = %cngn_amount,
            tx_hash = %transaction_hash.unwrap_or_default(),
            "Settlement completed successfully"
        );

        Ok(SettlementExecutionResult {
            settlement_id: settlement.id,
            user_id: settlement.user_id,
            fiat_amount: settlement.amount,
            cngn_amount,
            wallet_address,
            transaction_hash,
            status: settlement.settlement_status,
            executed_at: Utc::now(),
        })
    }

    /// Validate settlement amount is within limits
    fn validate_amount(&self, amount: Decimal) -> Result<(), SettlementError> {
        if amount < self.config.min_settlement_amount {
            return Err(SettlementError::AmountTooSmall {
                amount,
                min: self.config.min_settlement_amount,
            });
        }

        if amount > self.config.max_settlement_amount {
            return Err(SettlementError::AmountTooLarge {
                amount,
                max: self.config.max_settlement_amount,
            });
        }

        Ok(())
    }

    /// Calculate cNGN amount from fiat amount
    fn calculate_cngn_amount(&self, fiat_amount: Decimal) -> Decimal {
        fiat_amount * self.config.exchange_rate
    }

    /// Get user's non-custodial wallet address
    async fn get_user_wallet(&self, user_id: Uuid) -> Result<String, SettlementError> {
        // In production, would call wallet service
        // Generate mock Stellar address for now
        Ok(format!(
            "GD{}KL{}VI{}",
            user_id.to_string().replace("-", "").get(0..8).unwrap_or("USER"),
            user_id.to_string().replace("-", "").get(8..16).unwrap_or("WALLET"),
            user_id.to_string().replace("-", "").get(16..24).unwrap_or("ADDR")
        ))
    }

    /// Mint cNGN to user's wallet
    async fn mint_cngn(
        &self,
        wallet_address: &str,
        amount: Decimal,
    ) -> Result<Option<String>, SettlementError> {
        if !self.config.auto_mint_enabled {
            info!("Auto-mint disabled, skipping cNGN mint");
            return Ok(None);
        }

        info!(
            wallet = %wallet_address,
            amount = %amount,
            "Minting cNGN"
        );

        // In production, would call chain service to mint
        // For now, return mock transaction hash
        let tx_hash = format!(
            "abc{}def{}ghi",
            rand::random::<u64>(),
            rand::random::<u64>()
        );

        Ok(Some(tx_hash))
    }

    /// Handle settlement failure
    pub async fn handle_settlement_failure(
        &self,
        settlement: &mut FiatSettlement,
        error: &str,
    ) -> Result<(), SettlementError> {
        error!(settlement_id = %settlement.id, error = %error, "Settlement failed");

        settlement.settlement_status = SettlementStatus::Failed;
        settlement.settlement_error = Some(error.to_string());
        settlement.updated_at = Utc::now();

        // Could trigger alert or retry logic here

        Ok(())
    }
}

/// Settlement Errors
#[derive(Debug, thiserror::Error)]
pub enum SettlementError {
    #[error("Amount too small: {amount} (min: {min})")]
    AmountTooSmall { amount: Decimal, min: Decimal },

    #[error("Amount too large: {amount} (max: {max})")]
    AmountTooLarge { amount: Decimal, max: Decimal },

    #[error("Wallet not found for user")]
    WalletNotFound,

    #[error("Mint failed: {0}")]
    MintFailed(String),

    #[error("Invalid settlement state")]
    InvalidState,
}

impl std::fmt::Display for SettlementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettlementError::AmountTooSmall { amount, min } => {
                write!(f, "Amount {} below minimum {}", amount, min)
            }
            SettlementError::AmountTooLarge { amount, max } => {
                write!(f, "Amount {} exceeds maximum {}", amount, max)
            }
            SettlementError::WalletNotFound => write!(f, "Wallet not found"),
            SettlementError::MintFailed(s) => write!(f, "Mint failed: {}", s),
            SettlementError::InvalidState => write!(f, "Invalid settlement state"),
        }
    }
}