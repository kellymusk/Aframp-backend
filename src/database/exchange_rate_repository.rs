use crate::database::error::DatabaseError;
use crate::database::repository::{Repository, TransactionalRepository};
use async_trait::async_trait;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

/// Exchange Rate entity
#[derive(Debug, Clone, FromRow)]
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
}

impl ExchangeRateRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Get current exchange rate between two currencies
    pub async fn get_current_rate(
        &self,
        from_currency: &str,
        to_currency: &str,
    ) -> Result<Option<ExchangeRate>, DatabaseError> {
        sqlx::query_as::<_, ExchangeRate>(
            "SELECT id, from_currency, to_currency, rate, source, created_at, updated_at 
             FROM exchange_rates 
             WHERE from_currency = $1 AND to_currency = $2 
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(from_currency)
        .bind(to_currency)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
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
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Create or update exchange rate
    pub async fn upsert_rate(
        &self,
        from_currency: &str,
        to_currency: &str,
        rate: &str,
        source: Option<&str>,
    ) -> Result<ExchangeRate, DatabaseError> {
        let rate_id = Uuid::new_v4().to_string();

        sqlx::query_as::<_, ExchangeRate>(
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
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Get rates expiring soon (older than specified duration)
    pub async fn get_stale_rates(&self, hours_old: i32) -> Result<Vec<ExchangeRate>, DatabaseError> {
        sqlx::query_as::<_, ExchangeRate>(
            "SELECT id, from_currency, to_currency, rate, source, created_at, updated_at 
             FROM exchange_rates 
             WHERE updated_at < NOW() - INTERVAL '1 hour' * $1 
             ORDER BY updated_at ASC",
        )
        .bind(hours_old)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
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
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn find_all(&self) -> Result<Vec<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, ExchangeRate>(
            "SELECT id, from_currency, to_currency, rate, source, created_at, updated_at 
             FROM exchange_rates ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
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
        .map_err(|e| DatabaseError::from_sqlx(e))
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
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM exchange_rates WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| DatabaseError::from_sqlx(e))?;

        Ok(result.rows_affected() > 0)
    }
}

impl TransactionalRepository for ExchangeRateRepository {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
}
