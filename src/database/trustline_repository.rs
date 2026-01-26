use crate::database::error::DatabaseError;
use crate::database::repository::{Repository, TransactionalRepository};
use async_trait::async_trait;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

#[cfg(feature = "cache")]
use crate::cache::{cache::Cache, cache::RedisCache, keys::wallet::TrustlineKey};
#[cfg(feature = "cache")]
use tracing::debug;

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
    #[cfg(feature = "cache")]
    cache: Option<RedisCache>,
}

impl TrustlineRepository {
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

    /// Find trustline by account and asset
    /// Caches trustline existence for performance
    pub async fn find_trustline(
        &self,
        account: &str,
        asset_code: &str,
    ) -> Result<Option<Trustline>, DatabaseError> {
        // For AFRI trustlines, we cache whether the trustline exists and is active
        #[cfg(feature = "cache")]
        if let Some(ref cache) = self.cache {
            let trustline_key = TrustlineKey::new(account);
            if let Ok(Some(cached_exists)) =
                <RedisCache as Cache<bool>>::get::<'_, '_, '_>(cache, &trustline_key.to_string())
                    .await
            {
                if !cached_exists {
                    debug!("Cache hit: no trustline for account {}", account);
                    return Ok(None);
                }
                // If cache says it exists, we still need to fetch full data
                // This is a compromise for the common case where we just check existence
            }
        }

        let trustline = sqlx::query_as::<_, Trustline>(
            "SELECT id, account, asset_code, balance, limit, issuer, status, created_at, updated_at
             FROM trustlines
             WHERE account = $1 AND asset_code = $2",
        )
        .bind(account)
        .bind(asset_code)
        .fetch_optional(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)?;

        // Cache whether trustline exists (for existence checks)
        #[cfg(feature = "cache")]
        if let Some(ref cache) = &self.cache {
            let trustline_key = TrustlineKey::new(account);
            let exists = trustline.is_some();
            let ttl = crate::cache::cache::ttl::TRUSTLINES;
            if let Err(e) = cache
                .set(&trustline_key.to_string(), &exists, Some(ttl))
                .await
            {
                debug!("Failed to cache trustline existence: {}", e);
            } else {
                debug!(
                    "Cached trustline existence for account: {} ({})",
                    account, exists
                );
            }
        }

        Ok(trustline)
    }

    /// Find all trustlines for an account
    pub async fn find_by_account(&self, account: &str) -> Result<Vec<Trustline>, DatabaseError> {
        sqlx::query_as::<_, Trustline>(
            "SELECT id, account, asset_code, balance, limit, issuer, status, created_at, updated_at 
             FROM trustlines 
             WHERE account = $1 
             ORDER BY created_at DESC",
        )
        .bind(account)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
    }

    /// Create a new trustline
    /// Immediately caches positive trustline existence result
    pub async fn create_trustline(
        &self,
        account: &str,
        asset_code: &str,
        issuer: &str,
        limit: &str,
    ) -> Result<Trustline, DatabaseError> {
        let trustline_id = Uuid::new_v4().to_string();

        let trustline = sqlx::query_as::<_, Trustline>(
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
        .map_err(DatabaseError::from_sqlx)?;

        // Immediately cache that trustline exists for this account
        #[cfg(feature = "cache")]
        if let Some(ref cache) = self.cache {
            let trustline_key = TrustlineKey::new(account);
            let ttl = crate::cache::cache::ttl::TRUSTLINES;
            if let Err(e) = cache
                .set(&trustline_key.to_string(), &true, Some(ttl))
                .await
            {
                debug!("Failed to cache trustline creation: {}", e);
            } else {
                debug!("Cached trustline creation for account: {}", account);
            }
        }

        Ok(trustline)
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
        .map_err(DatabaseError::from_sqlx)
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
        .map_err(DatabaseError::from_sqlx)
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
                let balance: f64 = trustline.balance.parse().map_err(|_| {
                    DatabaseError::new(crate::database::error::DatabaseErrorKind::QueryError {
                        message: "Invalid balance format".to_string(),
                    })
                })?;
                let required: f64 = required_amount.parse().map_err(|_| {
                    DatabaseError::new(crate::database::error::DatabaseErrorKind::QueryError {
                        message: "Invalid amount format".to_string(),
                    })
                })?;

                Ok(balance >= required)
            }
            None => Ok(false),
        }
    }

    /// Find all active trustlines for asset
    pub async fn find_by_asset(&self, asset_code: &str) -> Result<Vec<Trustline>, DatabaseError> {
        sqlx::query_as::<_, Trustline>(
            "SELECT id, account, asset_code, balance, limit, issuer, status, created_at, updated_at 
             FROM trustlines 
             WHERE asset_code = $1 AND status = 'active' 
             ORDER BY created_at DESC",
        )
        .bind(asset_code)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
    }

    /// Delete a trustline by ID
    /// Also invalidates the trustline existence cache
    pub async fn delete(&self, trustline_id: &str) -> Result<bool, DatabaseError> {
        // First, get the trustline to retrieve account for cache invalidation
        let trustline = self.find_by_id(trustline_id).await?;

        let result = sqlx::query("DELETE FROM trustlines WHERE id = $1")
            .bind(trustline_id)
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::from_sqlx)?;

        let deleted = result.rows_affected() > 0;

        // Invalidate trustline existence cache if trustline was deleted
        #[cfg(feature = "cache")]
        if deleted {
            if let (Some(ref cache), Some(trustline_data)) = (&self.cache, trustline) {
                let trustline_key = TrustlineKey::new(&trustline_data.account);
                if let Err(e) = <RedisCache as Cache<bool>>::delete::<'_, '_, '_>(
                    cache,
                    &trustline_key.to_string(),
                )
                .await
                {
                    debug!("Failed to invalidate trustline cache on delete: {}", e);
                } else {
                    debug!(
                        "Invalidated trustline cache on delete: {}",
                        trustline_data.account
                    );
                }
            }
        }

        Ok(deleted)
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
        .map_err(DatabaseError::from_sqlx)
    }

    async fn find_all(&self) -> Result<Vec<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, Trustline>(
            "SELECT id, account, asset_code, balance, limit, issuer, status, created_at, updated_at 
             FROM trustlines ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
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
        .map_err(DatabaseError::from_sqlx)
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
        .map_err(DatabaseError::from_sqlx)
    }

    async fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM trustlines WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::from_sqlx)?;

        Ok(result.rows_affected() > 0)
    }
}

impl TransactionalRepository for TrustlineRepository {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
}
