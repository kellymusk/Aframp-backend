use crate::database::error::{DatabaseError, DatabaseErrorKind};
use crate::database::repository::{Repository, TransactionalRepository};
use async_trait::async_trait;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

/// Payment Provider Configuration entity
#[derive(Debug, Clone, FromRow)]
pub struct PaymentProviderConfig {
    pub provider: String,
    pub is_enabled: bool,
    pub settings: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Payment Method entity for storing user preferences
#[derive(Debug, Clone, FromRow)]
pub struct PaymentMethod {
    pub id: Uuid,
    pub user_id: Uuid,
    pub provider: String,
    pub method_type: String,
    pub phone_number: Option<String>,
    pub encrypted_data: Option<String>,
    pub is_active: bool,
    pub is_deleted: bool,
    pub region: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Repository for payment-related data
pub struct PaymentRepository {
    pool: PgPool,
}

impl PaymentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Find a provider configuration by provider name
    pub async fn get_provider_config(&self, provider: &str) -> Result<Option<PaymentProviderConfig>, DatabaseError> {
        sqlx::query_as::<_, PaymentProviderConfig>(
            "SELECT provider, is_enabled, settings, created_at, updated_at FROM payment_provider_configs WHERE provider = $1"
        )
        .bind(provider)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// List all payment methods for a user
    pub async fn find_by_user_id(&self, user_id: Uuid) -> Result<Vec<PaymentMethod>, DatabaseError> {
        sqlx::query_as::<_, PaymentMethod>(
            "SELECT id, user_id, provider, method_type, phone_number, encrypted_data, is_active, is_deleted, region, created_at, updated_at 
             FROM payment_methods 
             WHERE user_id = $1 AND is_deleted = false"
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Soft delete a payment method
    pub async fn soft_delete(&self, id: Uuid) -> Result<bool, DatabaseError> {
        let result = sqlx::query("UPDATE payment_methods SET is_deleted = true, is_active = false WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| DatabaseError::from_sqlx(e))?;
        Ok(result.rows_affected() > 0)
    }
}

#[async_trait]
impl Repository for PaymentRepository {
    type Entity = PaymentMethod;

    async fn find_by_id(&self, id: &str) -> Result<Option<Self::Entity>, DatabaseError> {
        let uuid = Uuid::parse_str(id).map_err(|e| DatabaseError::new(DatabaseErrorKind::Unknown { message: format!("Invalid UUID: {}", e) }))?;
        sqlx::query_as::<_, PaymentMethod>(
            "SELECT id, user_id, provider, method_type, phone_number, encrypted_data, is_active, is_deleted, region, created_at, updated_at 
             FROM payment_methods WHERE id = $1"
        )
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn find_all(&self) -> Result<Vec<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, PaymentMethod>(
            "SELECT id, user_id, provider, method_type, phone_number, encrypted_data, is_active, is_deleted, region, created_at, updated_at 
             FROM payment_methods ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn insert(&self, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        sqlx::query_as::<_, PaymentMethod>(
            "INSERT INTO payment_methods (user_id, provider, method_type, phone_number, encrypted_data, is_active, is_deleted, region) 
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) 
             RETURNING id, user_id, provider, method_type, phone_number, encrypted_data, is_active, is_deleted, region, created_at, updated_at"
        )
        .bind(entity.user_id)
        .bind(&entity.provider)
        .bind(&entity.method_type)
        .bind(&entity.phone_number)
        .bind(&entity.encrypted_data)
        .bind(entity.is_active)
        .bind(entity.is_deleted)
        .bind(&entity.region)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn update(&self, id: &str, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        let uuid = Uuid::parse_str(id).map_err(|e| DatabaseError::new(DatabaseErrorKind::Unknown { message: format!("Invalid UUID: {}", e) }))?;
        sqlx::query_as::<_, PaymentMethod>(
            "UPDATE payment_methods 
             SET user_id = $1, provider = $2, method_type = $3, phone_number = $4, encrypted_data = $5, is_active = $6, is_deleted = $7, region = $8 
             WHERE id = $9 
             RETURNING id, user_id, provider, method_type, phone_number, encrypted_data, is_active, is_deleted, region, created_at, updated_at"
        )
        .bind(entity.user_id)
        .bind(&entity.provider)
        .bind(&entity.method_type)
        .bind(&entity.phone_number)
        .bind(&entity.encrypted_data)
        .bind(entity.is_active)
        .bind(entity.is_deleted)
        .bind(&entity.region)
        .bind(uuid)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        let uuid = Uuid::parse_str(id).map_err(|e| DatabaseError::new(DatabaseErrorKind::Unknown { message: format!("Invalid UUID: {}", e) }))?;
        let result = sqlx::query("DELETE FROM payment_methods WHERE id = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await
            .map_err(|e| DatabaseError::from_sqlx(e))?;
        Ok(result.rows_affected() > 0)
    }
}

impl TransactionalRepository for PaymentRepository {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
}
