use sqlx::PgPool;
use uuid::Uuid;

use crate::blockchain::{keypair, wallet_crypto};
use crate::models::{NewWallet, Wallet};

#[derive(Debug, thiserror::Error)]
pub enum CreateWalletError {
    #[error("failed to generate wallet keypair: {0}")]
    Keygen(#[from] keypair::KeypairError),
    #[error("failed to encrypt wallet secret: {0}")]
    Encryption(String),
    #[error("merchant already has a wallet for network '{0}'")]
    DuplicateNetwork(String),
    /// The merchant already has a wallet — the `wallets_merchant_id_unique`
    /// constraint rejected the INSERT.  The API layer maps this to 409 Conflict
    /// rather than a 500, so the client can tell the merchant to use their
    /// existing wallet instead of retrying.
    #[error("a wallet already exists for this merchant")]
    AlreadyExists,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum DecryptWalletError {
    #[error("wallet not found")]
    NotFound,
    #[error("failed to decrypt wallet secret: {0}")]
    Decryption(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Fetches the encrypted secret seed for `wallet_id` from the database and
/// decrypts it using `key`.
///
/// This is intentionally separate from the public `Wallet` model, which never
/// exposes the encrypted secret — keeping the secret out of the read path
/// until it is explicitly needed.
///
/// # Usage
/// Currently only needed for the settlement/sweep feature (signing outbound
/// Stellar transactions on behalf of a merchant).  The `AppState` carries the
/// key; see the TODO comment there.
pub async fn decrypt_wallet_secret(
    db: &PgPool,
    wallet_id: Uuid,
    key: &[u8; 32],
) -> Result<String, DecryptWalletError> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT secret_key_encrypted FROM wallets WHERE id = $1",
    )
    .bind(wallet_id)
    .fetch_optional(db)
    .await?;

    let (secret_key_encrypted,) = row.ok_or(DecryptWalletError::NotFound)?;
    wallet_crypto::decrypt(key, &secret_key_encrypted)
        .map_err(DecryptWalletError::Decryption)
}

pub async fn create_wallet(
    db: &PgPool,
    merchant_id: Uuid,
    network: &str,
    encryption_key: &[u8; 32],
) -> Result<Wallet, CreateWalletError> {
    if wallet_by_merchant(db, merchant_id, network).await?.is_some() {
        return Err(CreateWalletError::DuplicateNetwork(network.to_string()));
    }

    let generated = keypair::generate_keypair();
    let generated = keypair::generate_keypair()?;
    let secret_key_encrypted = wallet_crypto::encrypt(encryption_key, &generated.secret_seed)
        .map_err(CreateWalletError::Encryption)?;

    let wallet = NewWallet {
        merchant_id,
        address: generated.public_address,
        network: network.to_string(),
        secret_key_encrypted,
    };
    sqlx::query_as::<_, Wallet>(
        "INSERT INTO wallets (merchant_id, address, network, secret_key_encrypted)
         VALUES ($1, $2, $3, $4)
         RETURNING id, merchant_id, address, network, created_at",
    )
    .bind(wallet.merchant_id)
    .bind(&wallet.address)
    .bind(&wallet.network)
    .bind(&wallet.secret_key_encrypted)
    .fetch_one(db)
    .await
    .map_err(|err| {
        // Postgres error code 23505 = unique_violation.  The
        // `wallets_merchant_id_unique` constraint means the merchant already
        // has a wallet; surface this as a distinct variant so the handler can
        // return 409 instead of 500.
        if let sqlx::Error::Database(ref db_err) = err {
            if db_err.code().as_deref() == Some("23505") {
                return CreateWalletError::AlreadyExists;
            }
        }
        CreateWalletError::Database(err)
    })
}

pub async fn wallet_by_merchant(
    db: &PgPool,
    merchant_id: Uuid,
    network: &str,
) -> Result<Option<Wallet>, sqlx::Error> {
    sqlx::query_as::<_, Wallet>(
        "SELECT id, merchant_id, address, network, created_at
           FROM wallets
          WHERE merchant_id = $1 AND network = $2
          ORDER BY created_at DESC
          LIMIT 1",
    )
    .bind(merchant_id)
    .bind(network)
    .fetch_optional(db)
    .await
}

pub async fn all_wallets(db: &PgPool) -> Result<Vec<Wallet>, sqlx::Error> {
    sqlx::query_as::<_, Wallet>(
        "SELECT id, merchant_id, address, network, created_at FROM wallets WHERE network = 'stellar'",
    )
    .fetch_all(db)
    .await
}

pub async fn wallet_by_id(db: &PgPool, id: Uuid) -> Result<Option<Wallet>, sqlx::Error> {
    sqlx::query_as::<_, Wallet>(
        "SELECT id, merchant_id, address, network, created_at FROM wallets WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn wallet_by_address(db: &PgPool, address: &str) -> Result<Option<Wallet>, sqlx::Error> {
    sqlx::query_as::<_, Wallet>(
        "SELECT id, merchant_id, address, network, created_at FROM wallets WHERE address = $1",
    )
    .bind(address)
    .fetch_optional(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockchain::wallet_crypto;

    /// Verifies that a secret encrypted with `wallet_crypto::encrypt` can be
    /// recovered by `wallet_crypto::decrypt` using the same key — the exact
    /// round-trip that `decrypt_wallet_secret` performs once the DB row is
    /// fetched.
    #[test]
    fn encrypt_decrypt_round_trips() {
        let key = [42u8; 32];
        let plaintext = "SCZANGBA5YHTNYVSKZF6V4YJLPGQZBTF7JGGZZZ5ZZZ"; // fake Stellar seed

        let encrypted = wallet_crypto::encrypt(&key, plaintext)
            .expect("encryption should succeed");
        let decrypted = wallet_crypto::decrypt(&key, &encrypted)
            .expect("decryption should succeed");

        assert_eq!(decrypted, plaintext);
    }

    /// Nonces are random, so two encryptions of the same plaintext must
    /// produce different ciphertext blobs.
    #[test]
    fn encrypt_is_nonce_randomised() {
        let key = [7u8; 32];
        let plaintext = "SCZANGBA5YHTNYVSKZF6V4YJLPGQZBTF7JGGZZZ5ZZZ";

        let a = wallet_crypto::encrypt(&key, plaintext).unwrap();
        let b = wallet_crypto::encrypt(&key, plaintext).unwrap();
        assert_ne!(a, b, "each encryption should use a fresh random nonce");
    }

    /// Decryption must fail if the key is wrong.
    #[test]
    fn decrypt_rejects_wrong_key() {
        let key_a = [1u8; 32];
        let key_b = [2u8; 32];
        let plaintext = "SCZANGBA5YHTNYVSKZF6V4YJLPGQZBTF7JGGZZZ5ZZZ";

        let encrypted = wallet_crypto::encrypt(&key_a, plaintext).unwrap();
        let result = wallet_crypto::decrypt(&key_b, &encrypted);
        assert!(result.is_err(), "wrong key should fail to decrypt");
    }
}
