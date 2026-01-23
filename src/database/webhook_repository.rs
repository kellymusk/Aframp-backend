use crate::database::error::{DatabaseError, DatabaseErrorKind};
use crate::database::repository::{Repository, TransactionalRepository};
use async_trait::async_trait;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

/// Webhook event entity
#[derive(Debug, Clone, FromRow)]
pub struct WebhookEvent {
    pub id: Uuid,
    pub event_id: String,
    pub provider: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub signature: Option<String>,
    pub status: String,
    pub transaction_id: Option<Uuid>,
    pub processed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub retry_count: i32,
    pub error_message: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
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
        event_id: &str,
        provider: &str,
        event_type: &str,
        payload: serde_json::Value,
        signature: Option<&str>,
        transaction_id: Option<Uuid>,
    ) -> Result<WebhookEvent, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "INSERT INTO webhook_events (event_id, provider, event_type, payload, signature, transaction_id, status) 
             VALUES ($1, $2, $3, $4, $5, $6, 'pending') 
             RETURNING id, event_id, provider, event_type, payload, signature, status, transaction_id, processed_at, retry_count, error_message, created_at, updated_at",
        )
        .bind(event_id)
        .bind(provider)
        .bind(event_type)
        .bind(payload)
        .bind(signature)
        .bind(transaction_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Get pending webhook events
    pub async fn get_pending_events(&self, limit: i64) -> Result<Vec<WebhookEvent>, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "SELECT id, event_id, provider, event_type, payload, signature, status, transaction_id, processed_at, retry_count, error_message, created_at, updated_at 
             FROM webhook_events 
             WHERE status = 'pending' AND retry_count < 5 
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
        id: Uuid,
    ) -> Result<WebhookEvent, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "UPDATE webhook_events SET status = 'completed', processed_at = NOW() WHERE id = $1 
             RETURNING id, event_id, provider, event_type, payload, signature, status, transaction_id, processed_at, retry_count, error_message, created_at, updated_at",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Record webhook processing failure
    pub async fn record_failure(
        &self,
        id: Uuid,
        error: &str,
    ) -> Result<WebhookEvent, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "UPDATE webhook_events 
             SET retry_count = retry_count + 1, error_message = $2, status = CASE WHEN retry_count + 1 >= 5 THEN 'failed' ELSE 'pending' END 
             WHERE id = $1 
             RETURNING id, event_id, provider, event_type, payload, signature, status, transaction_id, processed_at, retry_count, error_message, created_at, updated_at",
        )
        .bind(id)
        .bind(error)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Get webhook events by provider
    pub async fn find_by_provider(
        &self,
        provider: &str,
        limit: i64,
    ) -> Result<Vec<WebhookEvent>, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "SELECT id, event_id, provider, event_type, payload, signature, status, transaction_id, processed_at, retry_count, error_message, created_at, updated_at 
             FROM webhook_events 
             WHERE provider = $1 
             ORDER BY created_at DESC LIMIT $2",
        )
        .bind(provider)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Get failed webhook events
    pub async fn get_failed_events(&self, limit: i64) -> Result<Vec<WebhookEvent>, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "SELECT id, event_id, provider, event_type, payload, signature, status, transaction_id, processed_at, retry_count, error_message, created_at, updated_at 
             FROM webhook_events 
             WHERE status = 'failed' 
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
        let uuid = Uuid::parse_str(id).map_err(|e| DatabaseError::new(DatabaseErrorKind::Unknown { message: format!("Invalid UUID: {}", e) }))?;
        sqlx::query_as::<_, WebhookEvent>(
             "SELECT id, event_id, provider, event_type, payload, signature, status, transaction_id, processed_at, retry_count, error_message, created_at, updated_at 
             FROM webhook_events WHERE id = $1",
        )
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn find_all(&self) -> Result<Vec<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "SELECT id, event_id, provider, event_type, payload, signature, status, transaction_id, processed_at, retry_count, error_message, created_at, updated_at 
             FROM webhook_events ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn insert(&self, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        sqlx::query_as::<_, WebhookEvent>(
            "INSERT INTO webhook_events (event_id, provider, event_type, payload, signature, status, transaction_id, processed_at, retry_count, error_message, created_at) 
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) 
             RETURNING id, event_id, provider, event_type, payload, signature, status, transaction_id, processed_at, retry_count, error_message, created_at, updated_at",
        )
        .bind(&entity.event_id)
        .bind(&entity.provider)
        .bind(&entity.event_type)
        .bind(&entity.payload)
        .bind(&entity.signature)
        .bind(&entity.status)
        .bind(entity.transaction_id)
        .bind(entity.processed_at)
        .bind(entity.retry_count)
        .bind(&entity.error_message)
        .bind(entity.created_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn update(&self, id: &str, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        let uuid = Uuid::parse_str(id).map_err(|e| DatabaseError::new(DatabaseErrorKind::Unknown { message: format!("Invalid UUID: {}", e) }))?;
        sqlx::query_as::<_, WebhookEvent>(
            "UPDATE webhook_events 
             SET event_id = $1, provider = $2, event_type = $3, payload = $4, signature = $5, status = $6, transaction_id = $7, processed_at = $8, retry_count = $9, error_message = $10 
             WHERE id = $11 
             RETURNING id, event_id, provider, event_type, payload, signature, status, transaction_id, processed_at, retry_count, error_message, created_at, updated_at",
        )
        .bind(&entity.event_id)
        .bind(&entity.provider)
        .bind(&entity.event_type)
        .bind(&entity.payload)
        .bind(&entity.signature)
        .bind(&entity.status)
        .bind(entity.transaction_id)
        .bind(entity.processed_at)
        .bind(entity.retry_count)
        .bind(&entity.error_message)
        .bind(uuid)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        let uuid = Uuid::parse_str(id).map_err(|e| DatabaseError::new(DatabaseErrorKind::Unknown { message: format!("Invalid UUID: {}", e) }))?;
        let result = sqlx::query("DELETE FROM webhook_events WHERE id = $1")
            .bind(uuid)
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
