use crate::chains::stellar::client::StellarClient;
use crate::chains::stellar::config::StellarNetwork;
use crate::chains::stellar::errors::{StellarError, StellarResult};
use crate::chains::stellar::types::{is_valid_stellar_address, AssetBalance};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use stellar_strkey::ed25519::PublicKey as StrkeyPublicKey;
use stellar_xdr::next::{
    AccountId, AlphaNum12, AlphaNum4, AssetCode12, AssetCode4, ChangeTrustAsset, ChangeTrustOp,
    Limits, MuxedAccount, Operation, OperationBody, Preconditions, PublicKey, SequenceNumber,
    Transaction, TransactionEnvelope, TransactionExt, TransactionV1Envelope, Uint256, VecM,
    WriteXdr,
};

const BASE_RESERVE_XLM: f64 = 0.5;
const TRUSTLINE_RESERVE_XLM: f64 = 0.5;
const RECOMMENDED_FEE_BUFFER_XLM: f64 = 0.5;
const DEFAULT_BASE_FEE_STROOPS: u32 = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CngnAssetConfig {
    pub asset_code: String,
    pub issuer_testnet: String,
    pub issuer_mainnet: String,
    pub default_limit: Option<String>,
}

impl CngnAssetConfig {
    pub fn from_env() -> Self {
        Self {
            asset_code: std::env::var("CNGN_ASSET_CODE").unwrap_or_else(|_| "cNGN".to_string()),
            issuer_testnet: std::env::var("CNGN_ISSUER_TESTNET")
                .unwrap_or_else(|_| "GCNGN_TESTNET_ISSUER_PLACEHOLDER".to_string()),
            issuer_mainnet: std::env::var("CNGN_ISSUER_MAINNET")
                .unwrap_or_else(|_| "GCNGN_MAINNET_ISSUER_PLACEHOLDER".to_string()),
            default_limit: std::env::var("CNGN_DEFAULT_LIMIT").ok(),
        }
    }

    pub fn issuer_for_network(&self, network: &StellarNetwork) -> &str {
        match network {
            StellarNetwork::Testnet => &self.issuer_testnet,
            StellarNetwork::Mainnet => &self.issuer_mainnet,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustlineStatus {
    pub account_id: String,
    pub asset_code: String,
    pub issuer: String,
    pub has_trustline: bool,
    pub balance: Option<String>,
    pub limit: Option<String>,
    pub is_authorized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustlinePreflight {
    pub account_id: String,
    pub can_create: bool,
    pub available_xlm: String,
    pub required_xlm: String,
    pub recommended_min_xlm: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsignedTrustlineTransaction {
    pub account_id: String,
    pub asset_code: String,
    pub issuer: String,
    pub fee_stroops: u32,
    pub sequence: i64,
    pub transaction_hash: String,
    pub unsigned_envelope_xdr: String,
    pub limit: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CngnTrustlineManager {
    stellar_client: StellarClient,
    config: CngnAssetConfig,
}

impl CngnTrustlineManager {
    pub fn new(stellar_client: StellarClient) -> Self {
        Self {
            stellar_client,
            config: CngnAssetConfig::from_env(),
        }
    }

    pub fn with_config(stellar_client: StellarClient, config: CngnAssetConfig) -> Self {
        Self {
            stellar_client,
            config,
        }
    }

    pub fn asset_code(&self) -> &str {
        &self.config.asset_code
    }

    pub fn issuer(&self) -> &str {
        self.config
            .issuer_for_network(self.stellar_client.network())
    }

    pub async fn check_trustline(&self, account_id: &str) -> StellarResult<TrustlineStatus> {
        if !is_valid_stellar_address(account_id) {
            return Err(StellarError::invalid_address(account_id));
        }

        let account = self.stellar_client.get_account(account_id).await?;
        let issuer = self.issuer().to_string();
        let trustline = find_trustline(&account.balances, self.asset_code(), &issuer);

        Ok(match trustline {
            Some(balance) => TrustlineStatus {
                account_id: account_id.to_string(),
                asset_code: self.asset_code().to_string(),
                issuer,
                has_trustline: true,
                balance: Some(balance.balance.clone()),
                limit: balance.limit.clone(),
                is_authorized: balance.is_authorized,
            },
            None => TrustlineStatus {
                account_id: account_id.to_string(),
                asset_code: self.asset_code().to_string(),
                issuer,
                has_trustline: false,
                balance: None,
                limit: None,
                is_authorized: false,
            },
        })
    }

    pub async fn preflight_trustline_creation(
        &self,
        account_id: &str,
    ) -> StellarResult<TrustlinePreflight> {
        let account = self.stellar_client.get_account(account_id).await?;
        let available_xlm = account_xlm_balance(&account.balances);
        let required_xlm = required_xlm_for_trustline(account.subentry_count);
        let can_create = available_xlm >= required_xlm;

        Ok(TrustlinePreflight {
            account_id: account_id.to_string(),
            can_create,
            available_xlm: format!("{:.7}", available_xlm),
            required_xlm: format!("{:.7}", required_xlm),
            recommended_min_xlm: format!("{:.7}", required_xlm),
            reason: if can_create {
                None
            } else {
                Some(format!(
                    "Insufficient XLM for trustline reserve/fees. Need at least {:.7} XLM.",
                    required_xlm
                ))
            },
        })
    }

    pub async fn build_create_trustline_transaction(
        &self,
        account_id: &str,
        limit: Option<&str>,
        fee_stroops: Option<u32>,
    ) -> StellarResult<UnsignedTrustlineTransaction> {
        if !is_valid_stellar_address(account_id) {
            return Err(StellarError::invalid_address(account_id));
        }

        let status = self.check_trustline(account_id).await?;
        if status.has_trustline {
            return Err(StellarError::trustline_already_exists(
                account_id,
                self.asset_code(),
            ));
        }

        let preflight = self.preflight_trustline_creation(account_id).await?;
        if !preflight.can_create {
            return Err(StellarError::insufficient_xlm(
                preflight.available_xlm,
                preflight.required_xlm,
            ));
        }

        let account = self.stellar_client.get_account(account_id).await?;
        let fee = fee_stroops.unwrap_or(DEFAULT_BASE_FEE_STROOPS);
        let sequence = account.sequence + 1;
        let selected_limit = limit
            .map(|v| v.to_string())
            .or_else(|| self.config.default_limit.clone());
        let limit_i64 = match selected_limit.as_deref() {
            Some(raw_limit) => decimal_to_int64_stroops(raw_limit)?,
            None => i64::MAX,
        };

        let source = parse_muxed_account(account_id)?;
        let trustline_asset = build_change_trust_asset(self.asset_code(), self.issuer())?;
        let op = Operation {
            source_account: None,
            body: OperationBody::ChangeTrust(ChangeTrustOp {
                line: trustline_asset,
                limit: limit_i64,
            }),
        };

        let tx = Transaction {
            source_account: source,
            fee,
            seq_num: SequenceNumber(sequence),
            cond: Preconditions::None,
            memo: stellar_xdr::next::Memo::None,
            operations: VecM::try_from(vec![op])
                .map_err(|e| StellarError::serialization_error(e.to_string()))?,
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx: tx.clone(),
            signatures: VecM::try_from(Vec::new())
                .map_err(|e| StellarError::serialization_error(e.to_string()))?,
        });
        let network_id: [u8; 32] = Sha256::digest(
            self.stellar_client
                .network()
                .network_passphrase()
                .as_bytes(),
        )
        .into();
        let hash = tx
            .hash(network_id)
            .map_err(|e| StellarError::serialization_error(e.to_string()))?;
        let xdr = envelope
            .to_xdr_base64(Limits::none())
            .map_err(|e| StellarError::serialization_error(e.to_string()))?;

        Ok(UnsignedTrustlineTransaction {
            account_id: account_id.to_string(),
            asset_code: self.asset_code().to_string(),
            issuer: self.issuer().to_string(),
            fee_stroops: fee,
            sequence,
            transaction_hash: hex::encode(hash),
            unsigned_envelope_xdr: xdr,
            limit: selected_limit,
        })
    }

    pub async fn submit_signed_trustline_xdr(
        &self,
        signed_envelope_xdr: &str,
    ) -> StellarResult<serde_json::Value> {
        validate_signed_envelope_has_signatures(signed_envelope_xdr)?;
        self.stellar_client
            .submit_transaction_xdr(signed_envelope_xdr)
            .await
    }
}

fn validate_signed_envelope_has_signatures(xdr: &str) -> StellarResult<()> {
    use stellar_xdr::next::ReadXdr;
    let envelope = TransactionEnvelope::from_xdr_base64(xdr, Limits::none())
        .map_err(|e| StellarError::signing_error(format!("invalid envelope xdr: {}", e)))?;

    let signed = match envelope {
        TransactionEnvelope::Tx(v1) => !v1.signatures.is_empty(),
        TransactionEnvelope::TxV0(v0) => !v0.signatures.is_empty(),
        TransactionEnvelope::TxFeeBump(fb) => !fb.signatures.is_empty(),
    };

    if !signed {
        return Err(StellarError::signing_error(
            "signed envelope has no signatures",
        ));
    }

    Ok(())
}

fn find_trustline<'a>(
    balances: &'a [AssetBalance],
    asset_code: &str,
    issuer: &str,
) -> Option<&'a AssetBalance> {
    balances.iter().find(|balance| {
        matches!(
            balance.asset_type.as_str(),
            "credit_alphanum4" | "credit_alphanum12"
        ) && balance
            .asset_code
            .as_deref()
            .is_some_and(|code| code.eq_ignore_ascii_case(asset_code))
            && balance.asset_issuer.as_deref() == Some(issuer)
    })
}

fn account_xlm_balance(balances: &[AssetBalance]) -> f64 {
    balances
        .iter()
        .find(|b| b.asset_type == "native")
        .and_then(|b| b.balance.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn required_xlm_for_trustline(current_subentries: u32) -> f64 {
    (BASE_RESERVE_XLM * 2.0)
        + (current_subentries as f64 * TRUSTLINE_RESERVE_XLM)
        + TRUSTLINE_RESERVE_XLM
        + RECOMMENDED_FEE_BUFFER_XLM
}

fn parse_muxed_account(address: &str) -> StellarResult<MuxedAccount> {
    let public_key = StrkeyPublicKey::from_string(address)
        .map_err(|_| StellarError::invalid_address(address))?;
    Ok(MuxedAccount::Ed25519(Uint256(public_key.0)))
}

fn parse_account_id(address: &str) -> StellarResult<AccountId> {
    let public_key = StrkeyPublicKey::from_string(address)
        .map_err(|_| StellarError::invalid_address(address))?;
    Ok(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(
        public_key.0,
    ))))
}

fn build_change_trust_asset(asset_code: &str, issuer: &str) -> StellarResult<ChangeTrustAsset> {
    let issuer = parse_account_id(issuer)?;
    let code = asset_code.to_uppercase();
    let bytes = code.as_bytes();

    if code.is_empty() || code.len() > 12 {
        return Err(StellarError::config_error(
            "asset code must be between 1 and 12 chars",
        ));
    }

    if code.len() <= 4 {
        let mut code4 = [0u8; 4];
        code4[..bytes.len()].copy_from_slice(bytes);
        Ok(ChangeTrustAsset::CreditAlphanum4(AlphaNum4 {
            asset_code: AssetCode4(code4),
            issuer,
        }))
    } else {
        let mut code12 = [0u8; 12];
        code12[..bytes.len()].copy_from_slice(bytes);
        Ok(ChangeTrustAsset::CreditAlphanum12(AlphaNum12 {
            asset_code: AssetCode12(code12),
            issuer,
        }))
    }
}

fn decimal_to_int64_stroops(amount: &str) -> StellarResult<i64> {
    let value = amount.trim();
    if value.is_empty() {
        return Err(StellarError::transaction_failed("empty amount"));
    }
    if value.starts_with('-') {
        return Err(StellarError::transaction_failed(
            "negative values are not allowed",
        ));
    }

    let mut parts = value.split('.');
    let whole = parts.next().unwrap_or("0");
    let frac = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return Err(StellarError::transaction_failed(
            "invalid decimal amount format",
        ));
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return Err(StellarError::transaction_failed(
            "amount contains non-digit characters",
        ));
    }
    if frac.len() > 7 {
        return Err(StellarError::transaction_failed(
            "amount supports up to 7 decimal places",
        ));
    }
    let mut padded = frac.to_string();
    while padded.len() < 7 {
        padded.push('0');
    }

    let whole_i64: i64 = whole
        .parse()
        .map_err(|_| StellarError::transaction_failed("invalid amount"))?;
    let frac_i64: i64 = if padded.is_empty() {
        0
    } else {
        padded
            .parse()
            .map_err(|_| StellarError::transaction_failed("invalid amount"))?
    };

    whole_i64
        .checked_mul(10_000_000)
        .and_then(|v| v.checked_add(frac_i64))
        .ok_or_else(|| StellarError::transaction_failed("amount overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_required_xlm_for_trustline() {
        assert_eq!(required_xlm_for_trustline(0), 2.0);
        assert_eq!(required_xlm_for_trustline(2), 3.0);
    }

    #[test]
    fn test_decimal_to_int64_stroops() {
        assert_eq!(decimal_to_int64_stroops("1").unwrap(), 10_000_000);
        assert_eq!(decimal_to_int64_stroops("1.5").unwrap(), 15_000_000);
        assert!(decimal_to_int64_stroops("1.12345678").is_err());
    }
}
