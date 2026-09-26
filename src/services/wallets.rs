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

/// Fetches only the `address` column for all stellar wallets.
///
/// The deposit worker only needs addresses to call `fetch_deposits`, so this
/// avoids fetching and deserializing the remaining wallet columns on every poll
/// tick. Use [`all_wallets`] when the full wallet rows are required.
pub async fn all_wallet_addresses(db: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "SELECT address FROM wallets WHERE network = 'stellar'",
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

    #[sqlx::test]
    async fn all_wallet_addresses_returns_only_stellar_addresses(db: PgPool) {
        sqlx::query(
            "INSERT INTO wallets (merchant_id, address, network, secret_key_encrypted)
             VALUES
               ($1, 'GSTELLAR1', 'stellar', 'enc1'),
               ($1, 'GSTELLAR2', 'stellar', 'enc2'),
               ($1, 'GETH1', 'ethereum', 'enc3')",
        )
        .bind(Uuid::new_v4())
        .execute(&db)
        .await
        .expect("failed to seed wallets");

        let mut addresses = all_wallet_addresses(&db)
            .await
            .expect("all_wallet_addresses failed");
        addresses.sort();

        assert_eq!(addresses, vec!["GSTELLAR1".to_string(), "GSTELLAR2".to_string()]);
    }
}
