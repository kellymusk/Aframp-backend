use crate::database::error::DatabaseError;
use crate::database::repository::{Repository, TransactionalRepository};
use async_trait::async_trait;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

/// Webhook event entity
#[derive(Debug, Clone, FromRow)]
pub struct WebhookEvent {
    pub id: String,
    pub event_type: String,
    pub source: String,
    pub payload: serde_json::Value,
    pub processed: bool,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub processed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Webhook Repository for webhook event storage and tracking
pub struct WebhookRepository {
    pool: PgPool,
}

impl WebhookRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Log a new webhook event
    pub async fn log_event(
        &self,
        event_type: &str,
        source: &str,
        payload: serde_json::Value,
    ) -> Result<WebhookEvent, DatabaseError> {
        let event_id = Uuid::new_v4().to_string();

        sqlx::query_as::<_, WebhookEvent>(
            "INSERT INTO webhook_events (id, event_type, source, payload, processed, attempts, created_at) 
             VALUES ($1, $2, $3, $4, $5, $6, NOW()) 
             RETURNING id, event_type, source, payload, processed, attempts, last_error, created_at, processed_at",
        )
        .bind(&event_id)
        .bind(event_type)
        .bind(source)
        .bind(payload)
        .bind(false)
        .bind(0)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Get pending webhook events
    pub async fn get_pending_events(&self, limit: i64) -> Result<Vec<WebhookEvent>, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "SELECT id, event_type, source, payload, processed, attempts, last_error, created_at, processed_at 
             FROM webhook_events 
             WHERE processed = false AND attempts < 5 
             ORDER BY created_at ASC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Mark webhook event as processed
    pub async fn mark_processed(
        &self,
        event_id: &str,
    ) -> Result<WebhookEvent, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "UPDATE webhook_events SET processed = true, processed_at = NOW() WHERE id = $1 
             RETURNING id, event_type, source, payload, processed, attempts, last_error, created_at, processed_at",
        )
        .bind(event_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Record webhook processing failure
    pub async fn record_failure(
        &self,
        event_id: &str,
        error: &str,
    ) -> Result<WebhookEvent, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "UPDATE webhook_events 
             SET attempts = attempts + 1, last_error = $2 
             WHERE id = $1 
             RETURNING id, event_type, source, payload, processed, attempts, last_error, created_at, processed_at",
        )
        .bind(event_id)
        .bind(error)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Get webhook events by type
    pub async fn find_by_event_type(
        &self,
        event_type: &str,
        limit: i64,
    ) -> Result<Vec<WebhookEvent>, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "SELECT id, event_type, source, payload, processed, attempts, last_error, created_at, processed_at 
             FROM webhook_events 
             WHERE event_type = $1 
             ORDER BY created_at DESC LIMIT $2",
        )
        .bind(event_type)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Get failed webhook events
    pub async fn get_failed_events(&self, limit: i64) -> Result<Vec<WebhookEvent>, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "SELECT id, event_type, source, payload, processed, attempts, last_error, created_at, processed_at 
             FROM webhook_events 
             WHERE attempts >= 5 
             ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }
}

#[async_trait]
impl Repository for WebhookRepository {
    type Entity = WebhookEvent;

    async fn find_by_id(&self, id: &str) -> Result<Option<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "SELECT id, event_type, source, payload, processed, attempts, last_error, created_at, processed_at 
             FROM webhook_events WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn find_all(&self) -> Result<Vec<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "SELECT id, event_type, source, payload, processed, attempts, last_error, created_at, processed_at 
             FROM webhook_events ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn insert(&self, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "INSERT INTO webhook_events (id, event_type, source, payload, processed, attempts, last_error, created_at, processed_at) 
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) 
             RETURNING id, event_type, source, payload, processed, attempts, last_error, created_at, processed_at",
        )
        .bind(&entity.id)
        .bind(&entity.event_type)
        .bind(&entity.source)
        .bind(&entity.payload)
        .bind(entity.processed)
        .bind(entity.attempts)
        .bind(&entity.last_error)
        .bind(entity.created_at)
        .bind(entity.processed_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn update(&self, id: &str, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "UPDATE webhook_events 
             SET event_type = $1, source = $2, payload = $3, processed = $4, attempts = $5, last_error = $6, processed_at = $7 
             WHERE id = $8 
             RETURNING id, event_type, source, payload, processed, attempts, last_error, created_at, processed_at",
        )
        .bind(&entity.event_type)
        .bind(&entity.source)
        .bind(&entity.payload)
        .bind(entity.processed)
        .bind(entity.attempts)
        .bind(&entity.last_error)
        .bind(entity.processed_at)
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM webhook_events WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| DatabaseError::from_sqlx(e))?;

        Ok(result.rows_affected() > 0)
    }
}

impl TransactionalRepository for WebhookRepository {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
}
