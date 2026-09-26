use sqlx::PgPool;
use uuid::Uuid;

use crate::blockchain::{keypair, wallet_crypto};
use crate::models::{NewWallet, Wallet};

/// Purpose recorded in `wallet_secret_access_log` for decryptions done by
/// [`rotate_encryption_key`].
pub const PURPOSE_KEY_ROTATION: &str = "key_rotation";

#[derive(Debug, thiserror::Error)]
pub enum WalletSecretError {
    #[error("wallet not found")]
    NotFound,
    #[error("failed to decrypt wallet secret: {0}")]
    Decryption(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum CreateWalletError {
    #[error("failed to encrypt wallet secret: {0}")]
    Encryption(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

pub async fn create_wallet(
    db: &PgPool,
    merchant_id: Uuid,
    network: &str,
    encryption_key: &[u8; 32],
) -> Result<Wallet, CreateWalletError> {
    let generated = keypair::generate_keypair();
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
    .map_err(CreateWalletError::from)
}

pub async fn wallet_by_merchant(
    db: &PgPool,
    merchant_id: Uuid,
) -> Result<Option<Wallet>, sqlx::Error> {
    sqlx::query_as::<_, Wallet>(
        "SELECT id, merchant_id, address, network, created_at
           FROM wallets
          WHERE merchant_id = $1
          ORDER BY created_at DESC
          LIMIT 1",
    )
    .bind(merchant_id)
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

/// Decrypt a wallet's Stellar secret seed. Every call is recorded in
/// `wallet_secret_access_log` with `purpose` (e.g. `"sweep"`) before the
/// secret is decrypted, so each use of the encryption key leaves a trail.
pub async fn decrypt_secret(
    db: &PgPool,
    wallet_id: Uuid,
    encryption_key: &[u8; 32],
    purpose: &str,
) -> Result<String, WalletSecretError> {
    let encrypted: String =
        sqlx::query_scalar("SELECT secret_key_encrypted FROM wallets WHERE id = $1")
            .bind(wallet_id)
            .fetch_optional(db)
            .await?
            .ok_or(WalletSecretError::NotFound)?;

    log_secret_access(db, wallet_id, purpose).await?;
    tracing::info!(%wallet_id, purpose, "wallet secret decrypted");

    wallet_crypto::decrypt(encryption_key, &encrypted).map_err(WalletSecretError::Decryption)
}

async fn log_secret_access<'e, E>(executor: E, wallet_id: Uuid, purpose: &str) -> Result<(), sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query("INSERT INTO wallet_secret_access_log (wallet_id, purpose) VALUES ($1, $2)")
        .bind(wallet_id)
        .bind(purpose)
        .execute(executor)
        .await?;
    Ok(())
}

/// Re-encrypt every wallet secret from `old_key` to `new_key` in one
/// transaction: if any secret fails to decrypt under `old_key`, nothing is
/// changed. Each decryption is audit-logged as [`PURPOSE_KEY_ROTATION`].
/// Returns the number of wallets re-encrypted.
pub async fn rotate_encryption_key(
    db: &PgPool,
    old_key: &[u8; 32],
    new_key: &[u8; 32],
) -> Result<usize, WalletSecretError> {
    let mut tx = db.begin().await?;
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, secret_key_encrypted FROM wallets ORDER BY id FOR UPDATE")
            .fetch_all(&mut *tx)
            .await?;

    for (wallet_id, encrypted) in &rows {
        log_secret_access(&mut *tx, *wallet_id, PURPOSE_KEY_ROTATION).await?;
        let rotated = wallet_crypto::reencrypt(old_key, new_key, encrypted)
            .map_err(|e| WalletSecretError::Decryption(format!("wallet {wallet_id}: {e}")))?;
        sqlx::query("UPDATE wallets SET secret_key_encrypted = $1 WHERE id = $2")
            .bind(&rotated)
            .bind(wallet_id)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(rows.len())
}
