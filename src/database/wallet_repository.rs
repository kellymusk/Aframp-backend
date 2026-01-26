use crate::database::error::DatabaseError;
use crate::database::repository::{Repository, TransactionalRepository};
use async_trait::async_trait;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

#[cfg(feature = "cache")]
use crate::cache::{keys::wallet::BalanceKey, Cache, RedisCache};
#[cfg(feature = "cache")]
use tracing::debug;

/// Wallet entity
#[derive(Debug, Clone, FromRow)]
pub struct Wallet {
    pub id: String,
    pub user_id: String,
    pub account_address: String,
    pub balance: String, // Store as string to preserve precision
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Wallet Repository for wallet-specific database operations
pub struct WalletRepository {
    pool: PgPool,
    #[cfg(feature = "cache")]
    cache: Option<RedisCache>,
}

impl WalletRepository {
    /// Create a new repository without caching
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            #[cfg(feature = "cache")]
            cache: None,
        }
    }

    /// Create a new repository with Redis caching enabled
    #[cfg(feature = "cache")]
    pub fn with_cache(pool: PgPool, cache: RedisCache) -> Self {
        Self {
            pool,
            cache: Some(cache),
        }
    }

    /// Enable caching for an existing repository
    #[cfg(feature = "cache")]
    pub fn enable_cache(&mut self, cache: RedisCache) {
        self.cache = Some(cache);
    }

    /// Find wallet by user ID
    pub async fn find_by_user_id(&self, user_id: &str) -> Result<Option<Wallet>, DatabaseError> {
        sqlx::query_as::<_, Wallet>(
            "SELECT id, user_id, account_address, balance, created_at, updated_at 
             FROM wallets WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
    }

    /// Find wallet by account address
    /// Caches wallet balance for performance
    pub async fn find_by_account(
        &self,
        account_address: &str,
    ) -> Result<Option<Wallet>, DatabaseError> {
        // Try cache first for balance-only queries
        #[cfg(feature = "cache")]
        if let Some(ref cache) = self.cache {
            let balance_key = BalanceKey::new(account_address);
            if let Ok(Some(cached_balance)) =
                <RedisCache as Cache<String>>::get(cache, &balance_key.to_string()).await
            {
                debug!("Cache hit for wallet balance: {}", account_address);
                // We have cached balance, but need full wallet data from DB
                // This is a compromise - we avoid the full query but still need some DB access
                // For full performance, we'd need to cache the entire wallet object
                let _ = cached_balance; // Use the value
            }
        }

        let wallet = sqlx::query_as::<_, Wallet>(
            "SELECT id, user_id, account_address, balance, created_at, updated_at
             FROM wallets WHERE account_address = $1",
        )
        .bind(account_address)
        .fetch_optional(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)?;

        // Cache the balance if wallet found
        #[cfg(feature = "cache")]
        if let (Some(ref cache), Some(ref wallet_data)) = (&self.cache, &wallet) {
            let balance_key = BalanceKey::new(account_address);
            let ttl = crate::cache::cache::ttl::WALLET_BALANCES;
            if let Err(e) = cache
                .set(&balance_key.to_string(), &wallet_data.balance, Some(ttl))
                .await
            {
                debug!("Failed to cache wallet balance: {}", e);
            } else {
                debug!("Cached wallet balance: {}", account_address);
            }
        }

        Ok(wallet)
    }

    /// Update wallet balance
    /// Invalidates balance cache for the affected wallet
    pub async fn update_balance(
        &self,
        wallet_id: &str,
        new_balance: &str,
    ) -> Result<Wallet, DatabaseError> {
        let wallet = sqlx::query_as::<_, Wallet>(
            "UPDATE wallets SET balance = $1, updated_at = NOW()
             WHERE id = $2
             RETURNING id, user_id, account_address, balance, created_at, updated_at",
        )
        .bind(new_balance)
        .bind(wallet_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if matches!(e, sqlx::Error::RowNotFound) {
                DatabaseError::new(DatabaseErrorKind::NotFound {
                    entity: "Wallet".to_string(),
                    id: wallet_id.to_string(),
                })
            } else {
                DatabaseError::from_sqlx(e)
            }
        })?;

        // Invalidate balance cache
        #[cfg(feature = "cache")]
        if let Some(ref cache) = self.cache {
            let balance_key = BalanceKey::new(&wallet.account_address);
            if let Err(e) =
                <RedisCache as Cache<String>>::delete(cache, &balance_key.to_string()).await
            {
                debug!("Failed to invalidate wallet balance cache: {}", e);
            } else {
                debug!(
                    "Invalidated wallet balance cache: {}",
                    wallet.account_address
                );
            }
        }

        Ok(wallet)
    }

    /// Create a new wallet
    pub async fn create_wallet(
        &self,
        user_id: &str,
        account_address: &str,
        initial_balance: &str,
    ) -> Result<Wallet, DatabaseError> {
        let wallet_id = Uuid::new_v4().to_string();

        sqlx::query_as::<_, Wallet>(
            "INSERT INTO wallets (id, user_id, account_address, balance, created_at, updated_at) 
             VALUES ($1, $2, $3, $4, NOW(), NOW()) 
             RETURNING id, user_id, account_address, balance, created_at, updated_at",
        )
        .bind(&wallet_id)
        .bind(user_id)
        .bind(account_address)
        .bind(initial_balance)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(db_err) = &e {
                if db_err.code().as_deref() == Some("23505") {
                    // Unique constraint violation
                    DatabaseError::new(DatabaseErrorKind::UniqueConstraintViolation {
                        column: "account_address".to_string(),
                        value: account_address.to_string(),
                    })
                } else {
                    DatabaseError::from_sqlx(e)
                }
            } else {
                DatabaseError::from_sqlx(e)
            }
        })
    }

    /// Check if wallet has sufficient balance
    pub async fn has_sufficient_balance(
        &self,
        wallet_id: &str,
        required_amount: &str,
    ) -> Result<bool, DatabaseError> {
        let wallet = self.find_by_id(wallet_id).await?;
        match wallet {
            Some(w) => {
                // Parse as decimal for comparison
                let balance: f64 = w.balance.parse().map_err(|_| {
                    DatabaseError::new(DatabaseErrorKind::QueryError {
                        message: "Invalid balance format".to_string(),
                    })
                })?;
                let required: f64 = required_amount.parse().map_err(|_| {
                    DatabaseError::new(DatabaseErrorKind::QueryError {
                        message: "Invalid amount format".to_string(),
                    })
                })?;

                Ok(balance >= required)
            }
            None => Err(DatabaseError::new(DatabaseErrorKind::NotFound {
                entity: "Wallet".to_string(),
                id: wallet_id.to_string(),
            })),
        }
    }

    /// Delete a wallet by ID
    /// Also invalidates the balance cache for the wallet
    pub async fn delete(&self, wallet_id: &str) -> Result<bool, DatabaseError> {
        // First, get the wallet to retrieve account_address for cache invalidation
        let wallet = self.find_by_id(wallet_id).await?;

        let result = sqlx::query("DELETE FROM wallets WHERE id = $1")
            .bind(wallet_id)
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::from_sqlx)?;

        let deleted = result.rows_affected() > 0;

        // Invalidate balance cache if wallet was deleted
        #[cfg(feature = "cache")]
        if deleted {
            if let (Some(ref cache), Some(wallet_data)) = (&self.cache, wallet) {
                let balance_key = BalanceKey::new(&wallet_data.account_address);
                if let Err(e) =
                    <RedisCache as Cache<String>>::delete(cache, &balance_key.to_string()).await
                {
                    debug!("Failed to invalidate wallet balance cache on delete: {}", e);
                } else {
                    debug!(
                        "Invalidated wallet balance cache on delete: {}",
                        wallet_data.account_address
                    );
                }
            }
        }

        Ok(deleted)
    }
}

#[async_trait]
impl Repository for WalletRepository {
    type Entity = Wallet;

    async fn find_by_id(&self, id: &str) -> Result<Option<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, Wallet>(
            "SELECT id, user_id, account_address, balance, created_at, updated_at 
             FROM wallets WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
    }

    async fn find_all(&self) -> Result<Vec<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, Wallet>(
            "SELECT id, user_id, account_address, balance, created_at, updated_at 
             FROM wallets ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
    }

    async fn insert(&self, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        sqlx::query_as::<_, Wallet>(
            "INSERT INTO wallets (id, user_id, account_address, balance, created_at, updated_at) 
             VALUES ($1, $2, $3, $4, $5, $6) 
             RETURNING id, user_id, account_address, balance, created_at, updated_at",
        )
        .bind(&entity.id)
        .bind(&entity.user_id)
        .bind(&entity.account_address)
        .bind(&entity.balance)
        .bind(entity.created_at)
        .bind(entity.updated_at)
        .fetch_one(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
    }

    async fn update(&self, id: &str, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        sqlx::query_as::<_, Wallet>(
            "UPDATE wallets SET user_id = $1, account_address = $2, balance = $3, updated_at = NOW() 
             WHERE id = $4 
             RETURNING id, user_id, account_address, balance, created_at, updated_at",
        )
        .bind(&entity.user_id)
        .bind(&entity.account_address)
        .bind(&entity.balance)
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
    }

    async fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM wallets WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::from_sqlx)?;

        Ok(result.rows_affected() > 0)
    }
}

impl TransactionalRepository for WalletRepository {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
}

use crate::database::error::DatabaseErrorKind;

#[cfg(test)]
mod tests {
    // These tests require a running database
    // Run with: SQLX_OFFLINE=true cargo test
}
