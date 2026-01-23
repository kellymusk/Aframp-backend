use crate::database::error::DatabaseError;
use crate::database::repository::{Repository, TransactionalRepository};
use async_trait::async_trait;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

/// Trustline entity for AFRI trustline tracking
#[derive(Debug, Clone, FromRow)]
pub struct Trustline {
    pub id: String,
    pub account: String,
    pub asset_code: String,
    pub balance: String,
    pub limit: String,
    pub issuer: String,
    pub status: String, // "active", "pending", "revoked"
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Trustline Repository for AFRI trustline operations tracking
pub struct TrustlineRepository {
    pool: PgPool,
}

impl TrustlineRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Find trustline by account and asset
    pub async fn find_trustline(
        &self,
        account: &str,
        asset_code: &str,
    ) -> Result<Option<Trustline>, DatabaseError> {
        sqlx::query_as::<_, Trustline>(
            "SELECT id, account, asset_code, balance, limit, issuer, status, created_at, updated_at 
             FROM trustlines 
             WHERE account = $1 AND asset_code = $2",
        )
        .bind(account)
        .bind(asset_code)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Find all trustlines for an account
    pub async fn find_by_account(
        &self,
        account: &str,
    ) -> Result<Vec<Trustline>, DatabaseError> {
        sqlx::query_as::<_, Trustline>(
            "SELECT id, account, asset_code, balance, limit, issuer, status, created_at, updated_at 
             FROM trustlines 
             WHERE account = $1 
             ORDER BY created_at DESC",
        )
        .bind(account)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Create a new trustline
    pub async fn create_trustline(
        &self,
        account: &str,
        asset_code: &str,
        issuer: &str,
        limit: &str,
    ) -> Result<Trustline, DatabaseError> {
        let trustline_id = Uuid::new_v4().to_string();

        sqlx::query_as::<_, Trustline>(
            "INSERT INTO trustlines (id, account, asset_code, balance, limit, issuer, status, created_at, updated_at) 
             VALUES ($1, $2, $3, $4, $5, $6, $7, NOW(), NOW()) 
             RETURNING id, account, asset_code, balance, limit, issuer, status, created_at, updated_at",
        )
        .bind(&trustline_id)
        .bind(account)
        .bind(asset_code)
        .bind("0") // Initial balance
        .bind(limit)
        .bind(issuer)
        .bind("pending")
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Update trustline balance
    pub async fn update_balance(
        &self,
        trustline_id: &str,
        new_balance: &str,
    ) -> Result<Trustline, DatabaseError> {
        sqlx::query_as::<_, Trustline>(
            "UPDATE trustlines SET balance = $1, updated_at = NOW() 
             WHERE id = $2 
             RETURNING id, account, asset_code, balance, limit, issuer, status, created_at, updated_at",
        )
        .bind(new_balance)
        .bind(trustline_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Update trustline status
    pub async fn update_status(
        &self,
        trustline_id: &str,
        new_status: &str,
    ) -> Result<Trustline, DatabaseError> {
        sqlx::query_as::<_, Trustline>(
            "UPDATE trustlines SET status = $1, updated_at = NOW() 
             WHERE id = $2 
             RETURNING id, account, asset_code, balance, limit, issuer, status, created_at, updated_at",
        )
        .bind(new_status)
        .bind(trustline_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Check if account has sufficient AFRI balance
    pub async fn has_sufficient_balance(
        &self,
        account: &str,
        asset_code: &str,
        required_amount: &str,
    ) -> Result<bool, DatabaseError> {
        match self.find_trustline(account, asset_code).await? {
            Some(trustline) => {
                let balance: f64 = trustline.balance.parse()
                    .map_err(|_| DatabaseError::new(crate::database::error::DatabaseErrorKind::QueryError {
                        message: "Invalid balance format".to_string(),
                    }))?;
                let required: f64 = required_amount.parse()
                    .map_err(|_| DatabaseError::new(crate::database::error::DatabaseErrorKind::QueryError {
                        message: "Invalid amount format".to_string(),
                    }))?;

                Ok(balance >= required)
            }
            None => Ok(false),
        }
    }

    /// Find all active trustlines for asset
    pub async fn find_by_asset(
        &self,
        asset_code: &str,
    ) -> Result<Vec<Trustline>, DatabaseError> {
        sqlx::query_as::<_, Trustline>(
            "SELECT id, account, asset_code, balance, limit, issuer, status, created_at, updated_at 
             FROM trustlines 
             WHERE asset_code = $1 AND status = 'active' 
             ORDER BY created_at DESC",
        )
        .bind(asset_code)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }
}

#[async_trait]
impl Repository for TrustlineRepository {
    type Entity = Trustline;

    async fn find_by_id(&self, id: &str) -> Result<Option<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, Trustline>(
            "SELECT id, account, asset_code, balance, limit, issuer, status, created_at, updated_at 
             FROM trustlines WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn find_all(&self) -> Result<Vec<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, Trustline>(
            "SELECT id, account, asset_code, balance, limit, issuer, status, created_at, updated_at 
             FROM trustlines ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn insert(&self, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        sqlx::query_as::<_, Trustline>(
            "INSERT INTO trustlines (id, account, asset_code, balance, limit, issuer, status, created_at, updated_at) 
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) 
             RETURNING id, account, asset_code, balance, limit, issuer, status, created_at, updated_at",
        )
        .bind(&entity.id)
        .bind(&entity.account)
        .bind(&entity.asset_code)
        .bind(&entity.balance)
        .bind(&entity.limit)
        .bind(&entity.issuer)
        .bind(&entity.status)
        .bind(entity.created_at)
        .bind(entity.updated_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn update(&self, id: &str, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        sqlx::query_as::<_, Trustline>(
            "UPDATE trustlines 
             SET account = $1, asset_code = $2, balance = $3, limit = $4, issuer = $5, status = $6, updated_at = NOW() 
             WHERE id = $7 
             RETURNING id, account, asset_code, balance, limit, issuer, status, created_at, updated_at",
        )
        .bind(&entity.account)
        .bind(&entity.asset_code)
        .bind(&entity.balance)
        .bind(&entity.limit)
        .bind(&entity.issuer)
        .bind(&entity.status)
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM trustlines WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| DatabaseError::from_sqlx(e))?;

        Ok(result.rows_affected() > 0)
    }
}

impl TransactionalRepository for TrustlineRepository {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
}
