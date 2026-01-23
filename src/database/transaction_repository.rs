use crate::database::error::DatabaseError;
use crate::database::repository::{Repository, TransactionalRepository};
use async_trait::async_trait;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

/// Transaction status enum
#[derive(Debug, Clone, PartialEq, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
pub enum TransactionStatus {
    Pending,
    Processing,
    Completed,
    Failed,
    Cancelled,
}

/// Transaction entity
#[derive(Debug, Clone, FromRow)]
pub struct Transaction {
    pub id: String,
    pub wallet_id: String,
    pub transaction_type: String, // "onramp", "offramp", "payment"
    pub amount: String,
    pub status: String,
    pub fiat_amount: Option<String>,
    pub exchange_rate: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Transaction Repository for transaction-specific operations
pub struct TransactionRepository {
    pool: PgPool,
}

impl TransactionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Find transactions by wallet ID
    pub async fn find_by_wallet_id(
        &self,
        wallet_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Transaction>, DatabaseError> {
        sqlx::query_as::<_, Transaction>(
            "SELECT id, wallet_id, transaction_type, amount, status, fiat_amount, 
                    exchange_rate, metadata, created_at, updated_at 
             FROM transactions WHERE wallet_id = $1 
             ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(wallet_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Find transactions by status
    pub async fn find_by_status(
        &self,
        status: &str,
        limit: i64,
    ) -> Result<Vec<Transaction>, DatabaseError> {
        sqlx::query_as::<_, Transaction>(
            "SELECT id, wallet_id, transaction_type, amount, status, fiat_amount, 
                    exchange_rate, metadata, created_at, updated_at 
             FROM transactions WHERE status = $1 
             ORDER BY created_at ASC LIMIT $2",
        )
        .bind(status)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Update transaction status
    pub async fn update_status(
        &self,
        transaction_id: &str,
        new_status: &str,
    ) -> Result<Transaction, DatabaseError> {
        sqlx::query_as::<_, Transaction>(
            "UPDATE transactions SET status = $1, updated_at = NOW() 
             WHERE id = $2 
             RETURNING id, wallet_id, transaction_type, amount, status, fiat_amount, 
                      exchange_rate, metadata, created_at, updated_at",
        )
        .bind(new_status)
        .bind(transaction_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Create a new transaction
    pub async fn create_transaction(
        &self,
        wallet_id: &str,
        transaction_type: &str,
        amount: &str,
        fiat_amount: Option<&str>,
        exchange_rate: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) -> Result<Transaction, DatabaseError> {
        let transaction_id = Uuid::new_v4().to_string();

        sqlx::query_as::<_, Transaction>(
            "INSERT INTO transactions 
             (id, wallet_id, transaction_type, amount, status, fiat_amount, exchange_rate, metadata, created_at, updated_at) 
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW(), NOW()) 
             RETURNING id, wallet_id, transaction_type, amount, status, fiat_amount, 
                      exchange_rate, metadata, created_at, updated_at",
        )
        .bind(&transaction_id)
        .bind(wallet_id)
        .bind(transaction_type)
        .bind(amount)
        .bind("pending")
        .bind(fiat_amount)
        .bind(exchange_rate)
        .bind(metadata)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Get transaction count for a wallet
    pub async fn count_by_wallet(&self, wallet_id: &str) -> Result<i64, DatabaseError> {
        let result = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM transactions WHERE wallet_id = $1",
        )
        .bind(wallet_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))?;

        Ok(result)
    }
}

#[async_trait]
impl Repository for TransactionRepository {
    type Entity = Transaction;

    async fn find_by_id(&self, id: &str) -> Result<Option<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, Transaction>(
            "SELECT id, wallet_id, transaction_type, amount, status, fiat_amount, 
                    exchange_rate, metadata, created_at, updated_at 
             FROM transactions WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn find_all(&self) -> Result<Vec<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, Transaction>(
            "SELECT id, wallet_id, transaction_type, amount, status, fiat_amount, 
                    exchange_rate, metadata, created_at, updated_at 
             FROM transactions ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn insert(&self, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        sqlx::query_as::<_, Transaction>(
            "INSERT INTO transactions 
             (id, wallet_id, transaction_type, amount, status, fiat_amount, exchange_rate, metadata, created_at, updated_at) 
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) 
             RETURNING id, wallet_id, transaction_type, amount, status, fiat_amount, 
                      exchange_rate, metadata, created_at, updated_at",
        )
        .bind(&entity.id)
        .bind(&entity.wallet_id)
        .bind(&entity.transaction_type)
        .bind(&entity.amount)
        .bind(&entity.status)
        .bind(&entity.fiat_amount)
        .bind(&entity.exchange_rate)
        .bind(&entity.metadata)
        .bind(entity.created_at)
        .bind(entity.updated_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn update(&self, id: &str, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        sqlx::query_as::<_, Transaction>(
            "UPDATE transactions 
             SET wallet_id = $1, transaction_type = $2, amount = $3, status = $4, 
                 fiat_amount = $5, exchange_rate = $6, metadata = $7, updated_at = NOW() 
             WHERE id = $8 
             RETURNING id, wallet_id, transaction_type, amount, status, fiat_amount, 
                      exchange_rate, metadata, created_at, updated_at",
        )
        .bind(&entity.wallet_id)
        .bind(&entity.transaction_type)
        .bind(&entity.amount)
        .bind(&entity.status)
        .bind(&entity.fiat_amount)
        .bind(&entity.exchange_rate)
        .bind(&entity.metadata)
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM transactions WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| DatabaseError::from_sqlx(e))?;

        Ok(result.rows_affected() > 0)
    }
}

impl TransactionalRepository for TransactionRepository {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
}
