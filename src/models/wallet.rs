use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// The API- and JSON-facing view of a wallet.
///
/// **Never add `secret_key_encrypted` to this struct.** It derives
/// `Serialize`, so any field on it can end up in an HTTP response the
/// moment a handler wraps it in `Json(..)` — that's the whole reason this
/// struct exists as the *only* row shape used by wallet-returning API
/// queries. The encrypted Stellar seed lives in the `wallets` table but is
/// loaded only through [`WalletSecretRow`] / `services::wallets::wallet_secret_by_id`,
/// which nothing outside the blockchain module calls. If a feature needs
/// the encrypted secret (e.g. a future sweep-wallet operation), extend that
/// path — not this one.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Wallet {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub address: String,
    pub network: String,
    pub created_at: DateTime<Utc>,
}

/// Internal row shape carrying the encrypted secret seed. Deliberately does
/// **not** derive `Serialize` — the compiler rejects `Json(wallet_secret)`
/// outright rather than relying on every future caller remembering not to
/// serialize it. Used only by the blockchain module to decrypt a signing key
/// for outbound operations; never returned from an API handler.
#[derive(Debug, Clone, FromRow)]
pub struct WalletSecretRow {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub address: String,
    pub network: String,
    pub secret_key_encrypted: String,
}

#[derive(Debug, Clone)]
pub struct NewWallet {
    pub merchant_id: Uuid,
    pub address: String,
    pub network: String,
    pub secret_key_encrypted: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateWalletRequest {
    pub network: Option<String>,
}