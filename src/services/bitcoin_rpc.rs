//! Basic Bitcoin Core RPC integration: connect to a node, validate addresses,
//! read a balance via `scantxoutset`, and broadcast raw transactions. This is
//! the Bitcoin counterpart to `blockchain::stellar` — a thin, chain-specific
//! client that a future unified blockchain interface can wrap. No wallet
//! functionality (multi-sig, key management) lives here; the node is only
//! ever asked to look things up or relay something already signed.

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

pub type Satoshi = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitcoinNetwork {
    Mainnet,
    Testnet,
    Signet,
    Regtest,
}

impl BitcoinNetwork {
    pub fn from_str(s: &str) -> Result<Self, BtcError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "main" | "mainnet" => Ok(Self::Mainnet),
            "test" | "testnet" => Ok(Self::Testnet),
            "signet" => Ok(Self::Signet),
            "regtest" => Ok(Self::Regtest),
            other => Err(BtcError::Config(format!("unknown bitcoin network `{other}`"))),
        }
    }

    fn chain_name(self) -> &'static str {
        match self {
            Self::Mainnet => "main",
            Self::Testnet => "test",
            Self::Signet => "signet",
            Self::Regtest => "regtest",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BtcError {
    #[error("invalid bitcoin rpc configuration: {0}")]
    Config(String),
    #[error("failed to connect to bitcoin node: {0}")]
    ConnectionFailed(String),
    #[error("invalid bitcoin address: {0}")]
    InvalidAddress(String),
    #[error("failed to broadcast transaction: {0}")]
    BroadcastFailed(String),
    #[error("bitcoin rpc error: {0}")]
    Rpc(String),
}

pub struct BitcoinService {
    http: reqwest::Client,
    url: String,
    rpc_user: String,
    rpc_password: String,
    pub network: BitcoinNetwork,
}

impl BitcoinService {
    /// Connects to a Bitcoin Core node and verifies it's serving the
    /// expected network before handing back a usable client.
    pub async fn init_client(
        url: &str,
        rpc_user: &str,
        rpc_password: &str,
        network: BitcoinNetwork,
    ) -> Result<Self, BtcError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| BtcError::ConnectionFailed(e.to_string()))?;

        let service = Self {
            http,
            url: url.to_string(),
            rpc_user: rpc_user.to_string(),
            rpc_password: rpc_password.to_string(),
            network,
        };

        let info: BlockchainInfo = service
            .call("getblockchaininfo", json!([]))
            .await
            .map_err(|e| BtcError::ConnectionFailed(e.to_string()))?;
        if info.chain != network.chain_name() {
            return Err(BtcError::ConnectionFailed(format!(
                "node is on chain `{}`, expected `{}`",
                info.chain,
                network.chain_name()
            )));
        }

        Ok(service)
    }

    async fn call<T: DeserializeOwned>(&self, method: &str, params: serde_json::Value) -> Result<T, BtcError> {
        let response = self
            .http
            .post(&self.url)
            .basic_auth(&self.rpc_user, Some(&self.rpc_password))
            .json(&json!({
                "jsonrpc": "1.0",
                "id": "aframp",
                "method": method,
                "params": params,
            }))
            .send()
            .await
            .map_err(|e| BtcError::Rpc(e.to_string()))?;

        let raw: RpcResponse<T> = response
            .json()
            .await
            .map_err(|e| BtcError::Rpc(format!("unexpected rpc response: {e}")))?;

        if let Some(err) = raw.error {
            return Err(BtcError::Rpc(format!("{} (code {})", err.message, err.code)));
        }
        raw.result.ok_or_else(|| BtcError::Rpc("rpc response missing result".into()))
    }

    /// Structural validation only (Base58Check with checksum, or Bech32 v0
    /// with checksum) — doesn't require a node round-trip, so callers on a
    /// hot path aren't forced to pay RPC latency just to reject garbage
    /// input. `getblockchaininfo`-verified nodes can additionally call
    /// `validate_address_via_node` for the node's own authoritative check.
    pub fn validate_address(&self, address: &str) -> Result<bool, BtcError> {
        Ok(is_valid_base58_address(address, self.network) || is_valid_bech32_address(address, self.network))
    }

    /// Authoritative address validation via the node's own `validateaddress`
    /// RPC — slower than [`Self::validate_address`], but catches anything
    /// this crate's own (segwit-v0-only) decoder doesn't understand.
    pub async fn validate_address_via_node(&self, address: &str) -> Result<bool, BtcError> {
        let result: ValidateAddressResult = self.call("validateaddress", json!([address])).await?;
        Ok(result.is_valid)
    }

    /// Sums confirmed UTXOs for `address` via `scantxoutset`. This is a
    /// non-custodial lookup (no wallet import needed) so it works against
    /// any address, not just ones the node's wallet tracks.
    pub async fn get_balance(&self, address: &str) -> Result<Satoshi, BtcError> {
        if !self.validate_address(address)? {
            return Err(BtcError::InvalidAddress(address.to_string()));
        }
        let descriptor = format!("addr({address})");
        let result: ScanTxOutSetResult = self
            .call("scantxoutset", json!(["start", [descriptor]]))
            .await?;
        let btc = result.total_amount.unwrap_or(0.0);
        Ok((btc * 100_000_000.0).round() as Satoshi)
    }

    /// Relays an already-signed raw transaction. Returns the broadcast txid.
    pub async fn broadcast_tx(&self, raw_tx_hex: &str) -> Result<String, BtcError> {
        if raw_tx_hex.is_empty() || !raw_tx_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(BtcError::BroadcastFailed("raw transaction must be non-empty hex".into()));
        }
        self.call("sendrawtransaction", json!([raw_tx_hex]))
            .await
            .map_err(|e| BtcError::BroadcastFailed(e.to_string()))
    }
}

#[derive(Debug, Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcErrorObj>,
}

#[derive(Debug, Deserialize)]
struct RpcErrorObj {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct BlockchainInfo {
    chain: String,
}

#[derive(Debug, Deserialize)]
struct ScanTxOutSetResult {
    #[serde(default)]
    total_amount: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ValidateAddressResult {
    #[serde(rename = "isvalid")]
    is_valid: bool,
}

const BASE58_ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

fn base58_decode(input: &str) -> Option<Vec<u8>> {
    if input.is_empty() {
        return None;
    }
    let mut bytes = vec![0u8];
    for c in input.chars() {
        let value = BASE58_ALPHABET.iter().position(|&b| b as char == c)? as u32;
        let mut carry = value;
        for byte in bytes.iter_mut() {
            let x = (*byte as u32) * 58 + carry;
            *byte = (x & 0xff) as u8;
            carry = x >> 8;
        }
        while carry > 0 {
            bytes.push((carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    for c in input.chars() {
        if c == '1' {
            bytes.push(0);
        } else {
            break;
        }
    }
    bytes.reverse();
    Some(bytes)
}

/// Base58Check P2PKH/P2SH address, version-byte-gated to the given network.
fn is_valid_base58_address(address: &str, network: BitcoinNetwork) -> bool {
    let Some(decoded) = base58_decode(address) else {
        return false;
    };
    if decoded.len() < 5 {
        return false;
    }
    let (payload, checksum) = decoded.split_at(decoded.len() - 4);
    let hash1 = Sha256::digest(payload);
    let hash2 = Sha256::digest(hash1);
    if &hash2.as_slice()[..4] != checksum {
        return false;
    }
    let version = payload[0];
    match network {
        BitcoinNetwork::Mainnet => version == 0x00 || version == 0x05,
        BitcoinNetwork::Testnet | BitcoinNetwork::Signet | BitcoinNetwork::Regtest => {
            version == 0x6f || version == 0xc4
        }
    }
}

const BECH32_CHARSET: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

fn bech32_polymod(values: &[u8]) -> u32 {
    let gen = [0x3b6a57b2u32, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
    let mut chk: u32 = 1;
    for &v in values {
        let top = chk >> 25;
        chk = ((chk & 0x1ff_ffff) << 5) ^ (v as u32);
        for (i, g) in gen.iter().enumerate() {
            if (top >> i) & 1 == 1 {
                chk ^= *g;
            }
        }
    }
    chk
}

fn bech32_hrp_expand(hrp: &str) -> Vec<u8> {
    let mut v: Vec<u8> = hrp.bytes().map(|b| b >> 5).collect();
    v.push(0);
    v.extend(hrp.bytes().map(|b| b & 31));
    v
}

/// BIP173 (segwit v0) checksum verification only — this basic integration
/// doesn't need to distinguish v1+ (bech32m/Taproot) addresses, and rejecting
/// them here is the conservative choice for a "basic" first pass.
fn is_valid_bech32_address(address: &str, network: BitcoinNetwork) -> bool {
    if address.len() < 8 || address.len() > 90 {
        return false;
    }
    let lower = address.to_ascii_lowercase();
    if address != lower && address != address.to_ascii_uppercase() {
        return false;
    }
    let Some(pos) = lower.rfind('1') else {
        return false;
    };
    if pos == 0 || pos + 7 > lower.len() {
        return false;
    }
    let (hrp, rest) = lower.split_at(pos);
    let data_part = &rest[1..];

    let expected_hrp = match network {
        BitcoinNetwork::Mainnet => "bc",
        BitcoinNetwork::Testnet | BitcoinNetwork::Signet => "tb",
        BitcoinNetwork::Regtest => "bcrt",
    };
    if hrp != expected_hrp {
        return false;
    }

    let mut data = Vec::with_capacity(data_part.len());
    for c in data_part.chars() {
        let Some(v) = BECH32_CHARSET.iter().position(|&b| b as char == c) else {
            return false;
        };
        data.push(v as u8);
    }
    if data.len() < 6 {
        return false;
    }
    let mut values = bech32_hrp_expand(hrp);
    values.extend(data);
    bech32_polymod(&values) == 1
}
