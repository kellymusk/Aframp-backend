use sqlx::PgPool;
use uuid::Uuid;

use crate::blockchain::{keypair, wallet_crypto};
use crate::models::{NewWallet, Wallet};

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
         RETURNING id, merchant_id, address, network, created_at, last_polled_cursor",
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
        "SELECT id, merchant_id, address, network, created_at, last_polled_cursor
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
        "SELECT id, merchant_id, address, network, created_at, last_polled_cursor FROM wallets WHERE network = 'stellar'",
    )
    .fetch_all(db)
    .await
}

pub async fn wallet_by_id(db: &PgPool, id: Uuid) -> Result<Option<Wallet>, sqlx::Error> {
    sqlx::query_as::<_, Wallet>(
        "SELECT id, merchant_id, address, network, created_at, last_polled_cursor FROM wallets WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn wallet_by_address(db: &PgPool, address: &str) -> Result<Option<Wallet>, sqlx::Error> {
    sqlx::query_as::<_, Wallet>(
        "SELECT id, merchant_id, address, network, created_at, last_polled_cursor FROM wallets WHERE address = $1",
    )
    .bind(address)
    .fetch_optional(db)
    .await
}

pub async fn update_last_polled_cursor(
    db: &PgPool,
    wallet_id: Uuid,
    cursor: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE wallets SET last_polled_cursor = $1 WHERE id = $2")
        .bind(cursor)
        .bind(wallet_id)
        .execute(db)
        .await
        .map(|_| ())
}
