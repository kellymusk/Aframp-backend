use crate::chains::stellar::client::StellarClient;
use crate::chains::stellar::errors::StellarError;
use crate::chains::stellar::payment::{CngnMemo, CngnPaymentBuilder};
use crate::chains::traits::*;
use async_trait::async_trait;
use std::collections::HashMap;

/// Stellar implementation of the BlockchainService trait
pub struct StellarBlockchainService {
    client: StellarClient,
}

impl StellarBlockchainService {
    /// Create a new Stellar blockchain service
    pub fn new(client: StellarClient) -> Self {
        Self { client }
    }

    /// Get the underlying Stellar client
    pub fn client(&self) -> &StellarClient {
        &self.client
    }

    /// Get the payment builder
    fn payment_builder(&self) -> CngnPaymentBuilder {
        CngnPaymentBuilder::new(self.client.clone())
    }
}

/// Convert StellarError to BlockchainError
impl From<StellarError> for BlockchainError {
    fn from(error: StellarError) -> Self {
        match error {
            StellarError::AccountNotFound { address } => {
                BlockchainError::AccountNotFound { address }
            }
            StellarError::InvalidAddress { address } => BlockchainError::InvalidAddress { address },
            StellarError::NetworkError { message } => BlockchainError::NetworkError { message },
            StellarError::TransactionFailed { message } => {
                BlockchainError::TransactionFailed { message }
            }
            StellarError::TimeoutError { seconds } => BlockchainError::Timeout { seconds },
            StellarError::RateLimitError => BlockchainError::RateLimitExceeded,
            StellarError::InsufficientXlm {
                required,
                available,
            } => BlockchainError::InsufficientBalance {
                required,
                available,
            },
            StellarError::ConfigError { message } => BlockchainError::ConfigError { message },
            StellarError::SerializationError { message } => {
                BlockchainError::SerializationError { message }
            }
            StellarError::HealthCheckError { message } => BlockchainError::Other { message },
            StellarError::UnexpectedError { message } => BlockchainError::Other { message },
            StellarError::TrustlineAlreadyExists { address, asset } => BlockchainError::Other {
                message: format!("Trustline already exists for {} and {}", address, asset),
            },
            StellarError::SigningError { message } => BlockchainError::Other { message },
        }
    }
}

#[async_trait]
impl BlockchainService for StellarBlockchainService {
    fn chain_id(&self) -> &str {
        "stellar"
    }

    async fn account_exists(&self, address: &str) -> BlockchainResult<bool> {
        self.client
            .account_exists(address)
            .await
            .map_err(Into::into)
    }

    async fn get_account(&self, address: &str) -> BlockchainResult<AccountInfo> {
        let stellar_account = self.client.get_account(address).await?;

        let balances = stellar_account
            .balances
            .into_iter()
            .map(|b| AssetBalance {
                asset_code: b.asset_code.unwrap_or_else(|| "XLM".to_string()),
                issuer: b.asset_issuer,
                balance: b.balance,
                asset_type: b.asset_type,
                limit: b.limit,
            })
            .collect();

        let mut metadata = HashMap::new();
        metadata.insert("account_id".to_string(), stellar_account.account_id.clone());
        metadata.insert("sequence".to_string(), stellar_account.sequence.to_string());

        Ok(AccountInfo {
            address: stellar_account.account_id,
            sequence: stellar_account.sequence.to_string(),
            balances,
            metadata,
        })
    }

    async fn get_balances(&self, address: &str) -> BlockchainResult<Vec<AssetBalance>> {
        let account = self.get_account(address).await?;
        Ok(account.balances)
    }

    async fn get_asset_balance(
        &self,
        address: &str,
        asset_code: &str,
        issuer: Option<&str>,
    ) -> BlockchainResult<Option<String>> {
        self.client
            .get_asset_balance(address, asset_code, issuer)
            .await
            .map_err(Into::into)
    }

    async fn submit_transaction(&self, signed_tx: &str) -> BlockchainResult<TransactionResult> {
        let response = self.client.submit_transaction_xdr(signed_tx).await?;

        let hash = response
            .get("hash")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let successful = response
            .get("successful")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let ledger = response.get("ledger").and_then(|v| v.as_i64());

        let fee_charged = response
            .get("fee_charged")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(TransactionResult {
            hash,
            successful,
            ledger,
            fee_charged,
            raw_response: response,
        })
    }

    async fn get_transaction(&self, tx_hash: &str) -> BlockchainResult<TransactionResult> {
        let tx = self.client.get_transaction_by_hash(tx_hash).await?;

        Ok(TransactionResult {
            hash: tx.hash.clone(),
            successful: tx.successful,
            ledger: tx.ledger,
            fee_charged: tx.fee_charged.clone(),
            raw_response: serde_json::to_value(&tx).unwrap_or(serde_json::json!({})),
        })
    }

    async fn health_check(&self) -> BlockchainResult<ChainHealthStatus> {
        let health = self.client.health_check().await?;

        Ok(ChainHealthStatus {
            is_healthy: health.is_healthy,
            chain_id: "stellar".to_string(),
            response_time_ms: health.response_time_ms,
            last_check: health.last_check,
            error_message: health.error_message,
        })
    }

    fn validate_address(&self, address: &str) -> BlockchainResult<()> {
        if crate::chains::stellar::types::is_valid_stellar_address(address) {
            Ok(())
        } else {
            Err(BlockchainError::InvalidAddress {
                address: address.to_string(),
            })
        }
    }

    /// Estimate transaction fee for Stellar
    async fn estimate_fee(&self, _params: &TxParams) -> BlockchainResult<FeeEstimate> {
        // Stellar base fee is typically 100 stroops per operation
        // We estimate 2 operations (payment + source account)
        const DEFAULT_BASE_FEE: u32 = 100;
        let estimated_fee = DEFAULT_BASE_FEE * 2;

        // For non-native assets (like cNGN), might need trustline check
        let is_native = _params.asset_code == "XLM";

        Ok(FeeEstimate {
            fee: estimated_fee.to_string(),
            fee_unit: "stroops".to_string(),
            // Stellar typically confirms in 3-5 seconds
            estimated_confirmation_time_secs: if is_native { 5 } else { 10 },
            is_estimate: true,
        })
    }

    /// Build a Stellar transaction envelope (returns XDR)
    async fn build_transaction(
        &self,
        params: TxParams,
        source_address: &str,
    ) -> BlockchainResult<String> {
        // Validate addresses
        self.validate_address(source_address)?;
        self.validate_address(&params.to)?;

        // Build memo if provided
        let memo = params
            .memo
            .as_ref()
            .map(|m| {
                if m.is_empty() {
                    CngnMemo::None
                } else {
                    CngnMemo::Text(m.clone())
                }
            })
            .unwrap_or(CngnMemo::None);

        // Use the payment builder to create the transaction draft
        let draft = self
            .payment_builder()
            .build_payment(
                source_address,
                &params.to,
                &params.amount,
                memo,
                None, // Use default fee
            )
            .await
            .map_err(|e| BlockchainError::Other {
                message: format!("Failed to build payment: {}", e),
            })?;

        Ok(draft.unsigned_envelope_xdr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chains::stellar::config::{StellarConfig, StellarNetwork};

    #[tokio::test]
    async fn test_chain_id() {
        let config = StellarConfig {
            network: StellarNetwork::Testnet,
            ..Default::default()
        };
        let client = StellarClient::new(config).unwrap();
        let service = StellarBlockchainService::new(client);

        assert_eq!(service.chain_id(), "stellar");
    }

    #[tokio::test]
    async fn test_validate_address() {
        let config = StellarConfig {
            network: StellarNetwork::Testnet,
            ..Default::default()
        };
        let client = StellarClient::new(config).unwrap();
        let service = StellarBlockchainService::new(client);

        // Valid address
        assert!(service
            .validate_address("GCJRI5CIWK5IU67Q6DGA7QW52JDKRO7JEAHQKFNDUJUPEZGURDBX3LDX")
            .is_ok());

        // Invalid address
        assert!(service.validate_address("invalid").is_err());
    }

    #[tokio::test]
    async fn test_estimate_fee_native_asset() {
        let config = StellarConfig {
            network: StellarNetwork::Testnet,
            ..Default::default()
        };
        let client = StellarClient::new(config).unwrap();
        let service = StellarBlockchainService::new(client);

        let params = TxParams {
            to: "GCJRI5CIWK5IU67Q6DGA7QW52JDKRO7JEAHQKFNDUJUPEZGURDBX3LDX".to_string(),
            asset_code: "XLM".to_string(),
            issuer: None,
            amount: "100".to_string(),
            from: Some("GAQJF5G7E4L7G5F3J5C5XQZ7VY5RW3J3D5C5X5Z7VY5RW3J3D5C5XQZ7V".to_string()),
            memo: None,
        };

        let fee = service.estimate_fee(&params).await.unwrap();
        assert_eq!(fee.fee_unit, "stroops");
        assert!(!fee.fee.is_empty());
        assert_eq!(fee.estimated_confirmation_time_secs, 5);
        assert!(fee.is_estimate);
    }

    #[tokio::test]
    async fn test_estimate_fee_cngn_asset() {
        let config = StellarConfig {
            network: StellarNetwork::Testnet,
            ..Default::default()
        };
        let client = StellarClient::new(config).unwrap();
        let service = StellarBlockchainService::new(client);

        let params = TxParams {
            to: "GCJRI5CIWK5IU67Q6DGA7QW52JDKRO7JEAHQKFNDUJUPEZGURDBX3LDX".to_string(),
            asset_code: "cNGN".to_string(),
            issuer: Some("GAQJF5G7E4L7G5F3J5C5XQZ7VY5RW3J3D5C5X5Z7VY5RW3J3D5C5XQZ7V".to_string()),
            amount: "100".to_string(),
            from: Some("GAQJF5G7E4L7G5F3J5C5XQZ7VY5RW3J3D5C5X5Z7VY5RW3J3D5C5XQZ7V".to_string()),
            memo: Some("test payment".to_string()),
        };

        let fee = service.estimate_fee(&params).await.unwrap();
        assert_eq!(fee.fee_unit, "stroops");
        // cNGN may take longer due to trustline check
        assert_eq!(fee.estimated_confirmation_time_secs, 10);
    }

    #[tokio::test]
    async fn test_validate_address_muxed() {
        let config = StellarConfig {
            network: StellarNetwork::Testnet,
            ..Default::default()
        };
        let client = StellarClient::new(config).unwrap();
        let service = StellarBlockchainService::new(client);

        // Muxed address format
        let muxed_address = "MAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABGCTA2";
        // This should fail validation as it's not a valid public key format
        assert!(service.validate_address(muxed_address).is_err());
    }
}
