use crate::database::error::DatabaseError;
use crate::database::repository::{Repository, TransactionalRepository};
use async_trait::async_trait;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

/// Wallet entity
#[derive(Debug, Clone, FromRow)]
pub struct Wallet {
    pub id: String,
    pub user_id: String,
    pub account_address: String,
    pub balance: String,           // Store as string to preserve precision
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Wallet Repository for wallet-specific database operations
pub struct WalletRepository {
    pool: PgPool,
}

impl WalletRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
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
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Find wallet by account address
    pub async fn find_by_account(&self, account_address: &str) -> Result<Option<Wallet>, DatabaseError> {
        sqlx::query_as::<_, Wallet>(
            "SELECT id, user_id, account_address, balance, created_at, updated_at 
             FROM wallets WHERE account_address = $1",
        )
        .bind(account_address)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Update wallet balance
    pub async fn update_balance(
        &self,
        wallet_id: &str,
        new_balance: &str,
    ) -> Result<Wallet, DatabaseError> {
        sqlx::query_as::<_, Wallet>(
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
        })
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
                let balance: f64 = w.balance.parse()
                    .map_err(|_| DatabaseError::new(DatabaseErrorKind::QueryError {
                        message: "Invalid balance format".to_string(),
                    }))?;
                let required: f64 = required_amount.parse()
                    .map_err(|_| DatabaseError::new(DatabaseErrorKind::QueryError {
                        message: "Invalid amount format".to_string(),
                    }))?;

                Ok(balance >= required)
            }
            None => Err(DatabaseError::new(DatabaseErrorKind::NotFound {
                entity: "Wallet".to_string(),
                id: wallet_id.to_string(),
            })),
        }
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
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn find_all(&self) -> Result<Vec<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, Wallet>(
            "SELECT id, user_id, account_address, balance, created_at, updated_at 
             FROM wallets ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
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
        .map_err(|e| DatabaseError::from_sqlx(e))
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
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM wallets WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| DatabaseError::from_sqlx(e))?;

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
    use super::*;

    // These tests require a running database
    // Run with: SQLX_OFFLINE=true cargo test
}
