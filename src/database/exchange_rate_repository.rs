use crate::cache::keys::exchange_rate::CurrencyPairKey;
use crate::database::error::DatabaseError;
use crate::database::repository::{Repository, TransactionalRepository};
use async_trait::async_trait;
use serde::Deserialize;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

#[cfg(feature = "cache")]
use crate::cache::cache::Cache;
use crate::cache::cache::RedisCache;
#[cfg(feature = "cache")]
use tracing::debug;

/// Exchange Rate entity
#[derive(Debug, Clone, FromRow, serde::Serialize, Deserialize)]
pub struct ExchangeRate {
    pub id: String,
    pub from_currency: String,
    pub to_currency: String,
    pub rate: String,
    pub source: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Exchange Rate Repository for rate lookups and historical data
pub struct ExchangeRateRepository {
    pool: PgPool,
    #[cfg(feature = "cache")]
    cache: Option<RedisCache>,
}

impl ExchangeRateRepository {
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

    /// Get current exchange rate between two currencies
    /// Checks cache first, falls back to database, and caches the result
    pub async fn get_current_rate(
        &self,
        from_currency: &str,
        to_currency: &str,
    ) -> Result<Option<ExchangeRate>, DatabaseError> {
        let cache_key = CurrencyPairKey::new(from_currency, to_currency);

        // Try cache first
        #[cfg(feature = "cache")]
        if let Some(ref cache) = self.cache {
            if let Ok(Some(cached_rate)) = cache.get(&cache_key.to_string()).await {
                debug!(
                    "Cache hit for exchange rate: {} -> {}",
                    from_currency, to_currency
                );
                return Ok(Some(cached_rate));
            }
        }

        // Cache miss or no cache - query database
        let rate = sqlx::query_as::<_, ExchangeRate>(
            "SELECT id, from_currency, to_currency, rate, source, created_at, updated_at
             FROM exchange_rates
             WHERE from_currency = $1 AND to_currency = $2
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(from_currency)
        .bind(to_currency)
        .fetch_optional(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)?;

        // Cache the result if found
        #[cfg(feature = "cache")]
        if let (Some(ref cache), Some(ref rate_result)) = (&self.cache, &rate) {
            let ttl = crate::cache::cache::ttl::EXCHANGE_RATES;
            if let Err(e) = cache
                .set(&cache_key.to_string(), rate_result, Some(ttl))
                .await
            {
                debug!("Failed to cache exchange rate: {}", e);
                // Don't fail the operation if caching fails
            } else {
                debug!("Cached exchange rate: {} -> {}", from_currency, to_currency);
            }
        }

        Ok(rate)
    }

    /// Get historical rates between two currencies
    pub async fn get_historical_rates(
        &self,
        from_currency: &str,
        to_currency: &str,
        limit: i64,
    ) -> Result<Vec<ExchangeRate>, DatabaseError> {
        sqlx::query_as::<_, ExchangeRate>(
            "SELECT id, from_currency, to_currency, rate, source, created_at, updated_at 
             FROM exchange_rates 
             WHERE from_currency = $1 AND to_currency = $2 
             ORDER BY created_at DESC LIMIT $3",
        )
        .bind(from_currency)
        .bind(to_currency)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
    }

    /// Create or update exchange rate
    /// Invalidates cache for the affected currency pair
    pub async fn upsert_rate(
        &self,
        from_currency: &str,
        to_currency: &str,
        rate: &str,
        source: Option<&str>,
    ) -> Result<ExchangeRate, DatabaseError> {
        let rate_id = Uuid::new_v4().to_string();

        let result = sqlx::query_as::<_, ExchangeRate>(
            "INSERT INTO exchange_rates (id, from_currency, to_currency, rate, source, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, NOW(), NOW())
             ON CONFLICT (from_currency, to_currency)
             DO UPDATE SET rate = $4, source = $5, updated_at = NOW()
             RETURNING id, from_currency, to_currency, rate, source, created_at, updated_at",
        )
        .bind(&rate_id)
        .bind(from_currency)
        .bind(to_currency)
        .bind(rate)
        .bind(source)
        .fetch_one(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)?;

        // Invalidate cache for this currency pair
        #[cfg(feature = "cache")]
        if let Some(ref cache) = self.cache {
            let cache_key = CurrencyPairKey::new(from_currency, to_currency);
            if let Err(e) = <RedisCache as Cache<ExchangeRate>>::delete::<'_, '_, '_>(
                cache,
                &cache_key.to_string(),
            )
            .await
            {
                debug!("Failed to invalidate cache for exchange rate: {}", e);
                // Don't fail the operation if cache invalidation fails
            } else {
                debug!(
                    "Invalidated cache for exchange rate: {} -> {}",
                    from_currency, to_currency
                );
            }
        }

        Ok(result)
    }

    /// Get rates expiring soon (older than specified duration)
    pub async fn get_stale_rates(
        &self,
        hours_old: i32,
    ) -> Result<Vec<ExchangeRate>, DatabaseError> {
        sqlx::query_as::<_, ExchangeRate>(
            "SELECT id, from_currency, to_currency, rate, source, created_at, updated_at 
             FROM exchange_rates 
             WHERE updated_at < NOW() - INTERVAL '1 hour' * $1 
             ORDER BY updated_at ASC",
        )
        .bind(hours_old)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
    }
}

#[async_trait]
impl Repository for ExchangeRateRepository {
    type Entity = ExchangeRate;

    async fn find_by_id(&self, id: &str) -> Result<Option<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, ExchangeRate>(
            "SELECT id, from_currency, to_currency, rate, source, created_at, updated_at 
             FROM exchange_rates WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
    }

    async fn find_all(&self) -> Result<Vec<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, ExchangeRate>(
            "SELECT id, from_currency, to_currency, rate, source, created_at, updated_at 
             FROM exchange_rates ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
    }

    async fn insert(&self, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        sqlx::query_as::<_, ExchangeRate>(
            "INSERT INTO exchange_rates (id, from_currency, to_currency, rate, source, created_at, updated_at) 
             VALUES ($1, $2, $3, $4, $5, $6, $7) 
             RETURNING id, from_currency, to_currency, rate, source, created_at, updated_at",
        )
        .bind(&entity.id)
        .bind(&entity.from_currency)
        .bind(&entity.to_currency)
        .bind(&entity.rate)
        .bind(&entity.source)
        .bind(entity.created_at)
        .bind(entity.updated_at)
        .fetch_one(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
    }

    async fn update(&self, id: &str, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        sqlx::query_as::<_, ExchangeRate>(
            "UPDATE exchange_rates 
             SET from_currency = $1, to_currency = $2, rate = $3, source = $4, updated_at = NOW() 
             WHERE id = $5 
             RETURNING id, from_currency, to_currency, rate, source, created_at, updated_at",
        )
        .bind(&entity.from_currency)
        .bind(&entity.to_currency)
        .bind(&entity.rate)
        .bind(&entity.source)
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)
    }

    async fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM exchange_rates WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::from_sqlx)?;

        Ok(result.rows_affected() > 0)
    }
}

impl TransactionalRepository for ExchangeRateRepository {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
}
