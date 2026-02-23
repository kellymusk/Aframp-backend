use crate::chains::stellar::client::StellarClient;
use crate::chains::stellar::errors::{StellarError, StellarResult};
use crate::chains::stellar::trustline::CngnAssetConfig;
use crate::chains::stellar::types::{extract_asset_balance, is_valid_stellar_address};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use stellar_strkey::ed25519::{
    MuxedAccount as StrkeyMuxedAccount, PrivateKey as StrkeyPrivateKey,
    PublicKey as StrkeyPublicKey,
};
use stellar_xdr::next::{
    AccountId, AlphaNum12, AlphaNum4, Asset, AssetCode12, AssetCode4, DecoratedSignature, Hash,
    Limits, Memo, MuxedAccount, MuxedAccountMed25519, Operation, OperationBody, PaymentOp,
    Preconditions, PublicKey, ReadXdr, SequenceNumber, Signature, SignatureHint, StringM,
    TimeBounds, TimePoint, Transaction, TransactionEnvelope, TransactionExt, TransactionV1Envelope,
    Uint256, VecM, WriteXdr,
};

const DEFAULT_BASE_FEE_STROOPS: u32 = 100;
const DEFAULT_TIMEOUT_SECONDS: u64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CngnMemo {
    None,
    Text(String),
    Id(u64),
    Hash(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CngnPaymentDraft {
    pub source: String,
    pub destination: String,
    pub amount: String,
    pub asset_code: String,
    pub asset_issuer: String,
    pub sequence: i64,
    pub fee_stroops: u32,
    pub timeout_seconds: u64,
    pub created_at: String,
    pub transaction_hash: String,
    pub unsigned_envelope_xdr: String,
    pub memo: CngnMemo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedCngnPayment {
    pub draft: CngnPaymentDraft,
    pub signature: String,
    pub signed_envelope_xdr: String,
}

#[derive(Debug, Clone)]
pub struct CngnPaymentBuilder {
    stellar_client: StellarClient,
    config: CngnAssetConfig,
    base_fee_stroops: u32,
    timeout: Duration,
}

impl CngnPaymentBuilder {
    pub fn new(stellar_client: StellarClient) -> Self {
        Self {
            stellar_client,
            config: CngnAssetConfig::from_env(),
            base_fee_stroops: DEFAULT_BASE_FEE_STROOPS,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_base_fee(mut self, fee_stroops: u32) -> Self {
        self.base_fee_stroops = fee_stroops;
        self
    }

    pub async fn build_payment(
        &self,
        source: &str,
        destination: &str,
        amount: &str,
        memo: CngnMemo,
        fee_stroops: Option<u32>,
    ) -> StellarResult<CngnPaymentDraft> {
        validate_address(source)?;
        validate_address(destination)?;

        let source_account = self.stellar_client.get_account(source).await?;
        let destination_account = self.stellar_client.get_account(destination).await?;

        let issuer = self
            .config
            .issuer_for_network(self.stellar_client.network())
            .to_string();
        let asset_code = self.config.asset_code.clone();

        ensure_destination_has_trustline(&destination_account.balances, &asset_code, &issuer)?;

        let amount_stroops = decimal_to_stroops(amount)?;
        ensure_source_has_cngn_balance(
            &source_account.balances,
            amount_stroops,
            &asset_code,
            &issuer,
        )?;

        let fee = fee_stroops.unwrap_or(self.base_fee_stroops);
        ensure_source_has_xlm_for_fee(&source_account.balances, fee)?;

        let sequence = source_account.sequence + 1;
        let (tx, envelope) = build_unsigned_transaction(
            source,
            destination,
            amount_stroops,
            sequence,
            fee,
            self.timeout,
            &memo,
            &asset_code,
            &issuer,
        )?;

        let network_id = network_id(self.stellar_client.network().network_passphrase());
        let tx_hash = tx
            .hash(network_id)
            .map_err(|e| StellarError::serialization_error(e.to_string()))?;

        let unsigned_envelope_xdr = envelope
            .to_xdr_base64(Limits::none())
            .map_err(|e| StellarError::serialization_error(e.to_string()))?;

        Ok(CngnPaymentDraft {
            source: source.to_string(),
            destination: destination.to_string(),
            amount: amount.to_string(),
            asset_code,
            asset_issuer: issuer,
            sequence,
            fee_stroops: fee,
            timeout_seconds: self.timeout.as_secs(),
            created_at: chrono::Utc::now().to_rfc3339(),
            transaction_hash: hex::encode(tx_hash),
            unsigned_envelope_xdr,
            memo,
        })
    }

    pub fn sign_payment(
        &self,
        draft: CngnPaymentDraft,
        secret_seed: &str,
    ) -> StellarResult<SignedCngnPayment> {
        let signing_key = decode_signing_key(secret_seed)?;
        ensure_signing_key_matches_source(&signing_key, &draft.source)?;

        let envelope =
            TransactionEnvelope::from_xdr_base64(&draft.unsigned_envelope_xdr, Limits::none())
                .map_err(|e| StellarError::serialization_error(e.to_string()))?;

        let tx = match envelope {
            TransactionEnvelope::Tx(v1) => v1.tx,
            _ => {
                return Err(StellarError::signing_error(
                    "unsupported envelope type for cNGN payment",
                ))
            }
        };

        let network_id = network_id(self.stellar_client.network().network_passphrase());
        let hash = tx
            .hash(network_id)
            .map_err(|e| StellarError::serialization_error(e.to_string()))?;

        let signature_bytes = signing_key
            .try_sign(&hash)
            .map_err(|_| StellarError::signing_error("failed to sign transaction hash"))?
            .to_bytes()
            .to_vec();
        let hint = signature_hint(&signing_key)?;
        let signature = Signature::try_from(signature_bytes.clone())
            .map_err(|e| StellarError::serialization_error(e.to_string()))?;
        let decorated = DecoratedSignature { hint, signature };
        let signed_env = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::try_from(vec![decorated])
                .map_err(|e| StellarError::serialization_error(e.to_string()))?,
        });
        let signed_envelope_xdr = signed_env
            .to_xdr_base64(Limits::none())
            .map_err(|e| StellarError::serialization_error(e.to_string()))?;

        Ok(SignedCngnPayment {
            draft,
            signature: hex::encode(signature_bytes),
            signed_envelope_xdr,
        })
    }

    pub async fn submit_signed_payment(
        &self,
        signed_envelope_xdr: &str,
    ) -> StellarResult<serde_json::Value> {
        validate_signed_envelope_has_signatures(signed_envelope_xdr)?;
        self.stellar_client
            .submit_transaction_xdr(signed_envelope_xdr)
            .await
    }
}

fn validate_address(address: &str) -> StellarResult<()> {
    if is_valid_stellar_address(address) {
        Ok(())
    } else {
        Err(StellarError::invalid_address(address))
    }
}

fn ensure_destination_has_trustline(
    balances: &[crate::chains::stellar::types::AssetBalance],
    asset_code: &str,
    issuer: &str,
) -> StellarResult<()> {
    if extract_asset_balance(balances, asset_code, Some(issuer)).is_some() {
        Ok(())
    } else {
        Err(StellarError::transaction_failed(
            "recipient has no cNGN trustline (op_no_trust)",
        ))
    }
}

fn ensure_source_has_xlm_for_fee(
    balances: &[crate::chains::stellar::types::AssetBalance],
    fee_stroops: u32,
) -> StellarResult<()> {
    let available = balances
        .iter()
        .find(|b| b.asset_type == "native")
        .and_then(|b| b.balance.parse::<f64>().ok())
        .unwrap_or(0.0);
    let required = (fee_stroops as f64) / 10_000_000.0;
    if available >= required {
        Ok(())
    } else {
        Err(StellarError::insufficient_xlm(
            format!("{:.7} XLM", available),
            format!("{:.7} XLM", required),
        ))
    }
}

fn ensure_source_has_cngn_balance(
    balances: &[crate::chains::stellar::types::AssetBalance],
    amount_stroops: i64,
    asset_code: &str,
    issuer: &str,
) -> StellarResult<()> {
    let balance = extract_asset_balance(balances, asset_code, Some(issuer))
        .unwrap_or_else(|| "0".to_string());
    let available_stroops = decimal_to_stroops(&balance)?;
    if available_stroops >= amount_stroops {
        Ok(())
    } else {
        Err(StellarError::transaction_failed(format!(
            "insufficient cNGN balance: available={}, required={}",
            balance,
            decimal_from_stroops(amount_stroops)
        )))
    }
}

fn build_unsigned_transaction(
    source: &str,
    destination: &str,
    amount_stroops: i64,
    sequence: i64,
    fee_stroops: u32,
    timeout: Duration,
    memo: &CngnMemo,
    asset_code: &str,
    issuer: &str,
) -> StellarResult<(Transaction, TransactionEnvelope)> {
    let source_account = parse_muxed_account(source)?;
    let destination_account = parse_muxed_account(destination)?;
    let asset = build_asset(asset_code, issuer)?;

    let op = Operation {
        source_account: None,
        body: OperationBody::Payment(PaymentOp {
            destination: destination_account,
            asset,
            amount: amount_stroops,
        }),
    };

    let now = unix_time();
    let tx = Transaction {
        source_account: source_account,
        fee: fee_stroops,
        seq_num: SequenceNumber(sequence),
        cond: Preconditions::Time(TimeBounds {
            min_time: TimePoint(now),
            max_time: TimePoint(now + timeout.as_secs()),
        }),
        memo: memo_to_xdr(memo)?,
        operations: VecM::try_from(vec![op])
            .map_err(|e| StellarError::serialization_error(e.to_string()))?,
        ext: TransactionExt::V0,
    };

    let env = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx: tx.clone(),
        signatures: VecM::try_from(Vec::<DecoratedSignature>::new())
            .map_err(|e| StellarError::serialization_error(e.to_string()))?,
    });
    Ok((tx, env))
}

fn parse_muxed_account(address: &str) -> StellarResult<MuxedAccount> {
    if address.starts_with('M') {
        let muxed = StrkeyMuxedAccount::from_string(address)
            .map_err(|_| StellarError::invalid_address(address))?;
        Ok(MuxedAccount::MuxedEd25519(MuxedAccountMed25519 {
            id: muxed.id,
            ed25519: Uint256(muxed.ed25519),
        }))
    } else {
        let public_key = StrkeyPublicKey::from_string(address)
            .map_err(|_| StellarError::invalid_address(address))?;
        Ok(MuxedAccount::Ed25519(Uint256(public_key.0)))
    }
}

fn parse_account_id(address: &str) -> StellarResult<AccountId> {
    let public_key = StrkeyPublicKey::from_string(address)
        .map_err(|_| StellarError::invalid_address(address))?;
    Ok(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(
        public_key.0,
    ))))
}

fn build_asset(asset_code: &str, issuer: &str) -> StellarResult<Asset> {
    let issuer = parse_account_id(issuer)?;
    let code = asset_code.trim().to_uppercase();
    let bytes = code.as_bytes();
    if code.is_empty() || code.len() > 12 {
        return Err(StellarError::config_error(
            "asset code must be 1..=12 characters",
        ));
    }

    if code.len() <= 4 {
        let mut v = [0u8; 4];
        v[..bytes.len()].copy_from_slice(bytes);
        Ok(Asset::CreditAlphanum4(AlphaNum4 {
            asset_code: AssetCode4(v),
            issuer,
        }))
    } else {
        let mut v = [0u8; 12];
        v[..bytes.len()].copy_from_slice(bytes);
        Ok(Asset::CreditAlphanum12(AlphaNum12 {
            asset_code: AssetCode12(v),
            issuer,
        }))
    }
}

fn memo_to_xdr(memo: &CngnMemo) -> StellarResult<Memo> {
    match memo {
        CngnMemo::None => Ok(Memo::None),
        CngnMemo::Text(text) => {
            if text.as_bytes().len() > 28 {
                return Err(StellarError::transaction_failed(
                    "memo text must be <= 28 bytes",
                ));
            }
            let v: StringM<28> = text
                .parse::<StringM<28>>()
                .map_err(|e| StellarError::serialization_error(e.to_string()))?;
            Ok(Memo::Text(v))
        }
        CngnMemo::Id(v) => Ok(Memo::Id(*v)),
        CngnMemo::Hash(v) => {
            let h: Hash = v
                .parse()
                .map_err(|_| StellarError::transaction_failed("memo hash must be 32-byte hex"))?;
            Ok(Memo::Hash(h))
        }
    }
}

fn decimal_to_stroops(amount: &str) -> StellarResult<i64> {
    let trimmed = amount.trim();
    if trimmed.is_empty() {
        return Err(StellarError::transaction_failed("amount is required"));
    }
    if trimmed.starts_with('-') {
        return Err(StellarError::transaction_failed(
            "amount must be greater than zero",
        ));
    }
    let mut parts = trimmed.split('.');
    let whole = parts.next().unwrap_or("0");
    let frac = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return Err(StellarError::transaction_failed(
            "invalid amount decimal format",
        ));
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return Err(StellarError::transaction_failed(
            "amount contains non-digit characters",
        ));
    }
    if frac.len() > 7 {
        return Err(StellarError::transaction_failed(
            "amount supports at most 7 decimals",
        ));
    }
    let mut frac_padded = frac.to_string();
    while frac_padded.len() < 7 {
        frac_padded.push('0');
    }

    let whole_i64: i64 = whole
        .parse()
        .map_err(|_| StellarError::transaction_failed("invalid amount"))?;
    let frac_i64: i64 = frac_padded
        .parse()
        .map_err(|_| StellarError::transaction_failed("invalid amount"))?;

    whole_i64
        .checked_mul(10_000_000)
        .and_then(|v| v.checked_add(frac_i64))
        .ok_or_else(|| StellarError::transaction_failed("amount overflow"))
}

fn decimal_from_stroops(stroops: i64) -> String {
    let whole = stroops / 10_000_000;
    let frac = (stroops % 10_000_000).abs();
    format!("{whole}.{frac:07}")
}

fn decode_signing_key(secret_seed: &str) -> StellarResult<SigningKey> {
    let private = StrkeyPrivateKey::from_string(secret_seed)
        .map_err(|_| StellarError::signing_error("invalid secret seed"))?;
    Ok(SigningKey::from_bytes(&private.0))
}

fn ensure_signing_key_matches_source(signing_key: &SigningKey, source: &str) -> StellarResult<()> {
    let public_key_bytes = signing_key.verifying_key().to_bytes();
    let expected = if source.starts_with('M') {
        StrkeyMuxedAccount::from_string(source)
            .map(|m| m.ed25519)
            .map_err(|_| StellarError::invalid_address(source))?
    } else {
        StrkeyPublicKey::from_string(source)
            .map(|p| p.0)
            .map_err(|_| StellarError::invalid_address(source))?
    };

    if public_key_bytes == expected {
        Ok(())
    } else {
        Err(StellarError::signing_error(
            "secret seed does not match source account",
        ))
    }
}

fn signature_hint(signing_key: &SigningKey) -> StellarResult<SignatureHint> {
    let bytes = signing_key.verifying_key().to_bytes();
    SignatureHint::try_from(&bytes[bytes.len() - 4..])
        .map_err(|e| StellarError::serialization_error(e.to_string()))
}

fn network_id(passphrase: &str) -> [u8; 32] {
    Sha256::digest(passphrase.as_bytes()).into()
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn validate_signed_envelope_has_signatures(xdr: &str) -> StellarResult<()> {
    use stellar_xdr::next::ReadXdr;
    let env = TransactionEnvelope::from_xdr_base64(xdr, Limits::none())
        .map_err(|e| StellarError::signing_error(format!("invalid xdr: {}", e)))?;
    let has_sigs = match env {
        TransactionEnvelope::Tx(v1) => !v1.signatures.is_empty(),
        TransactionEnvelope::TxV0(v0) => !v0.signatures.is_empty(),
        TransactionEnvelope::TxFeeBump(fb) => !fb.signatures.is_empty(),
    };
    if has_sigs {
        Ok(())
    } else {
        Err(StellarError::signing_error(
            "signed envelope has no signatures",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decimal_to_stroops_ok() {
        assert_eq!(decimal_to_stroops("1").unwrap(), 10_000_000);
        assert_eq!(decimal_to_stroops("1.2500000").unwrap(), 12_500_000);
    }

    #[test]
    fn test_decimal_to_stroops_invalid() {
        assert!(decimal_to_stroops("-1").is_err());
        assert!(decimal_to_stroops("1.12345678").is_err());
        assert!(decimal_to_stroops("abc").is_err());
    }
}
