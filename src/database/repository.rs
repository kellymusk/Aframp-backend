use crate::database::error::DatabaseError;
use async_trait::async_trait;

/// Base repository trait defining common database operations
/// All domain-specific repositories should implement this trait
#[async_trait]
pub trait Repository: Send + Sync {
    /// Associated type for the entity this repository manages
    type Entity: Send + Sync;

    /// Find an entity by its ID
    async fn find_by_id(&self, id: &str) -> Result<Option<Self::Entity>, DatabaseError>;

    /// Find all entities matching a criteria (basic implementation)
    async fn find_all(&self) -> Result<Vec<Self::Entity>, DatabaseError>;

    /// Insert a new entity
    async fn insert(&self, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError>;

    /// Update an existing entity
    async fn update(&self, id: &str, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError>;

    /// Delete an entity by ID
    async fn delete(&self, id: &str) -> Result<bool, DatabaseError>;

    /// Check if an entity exists by ID
    async fn exists(&self, id: &str) -> Result<bool, DatabaseError> {
        match self.find_by_id(id).await {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// Trait for repositories that support transactions
#[async_trait]
pub trait TransactionalRepository: Repository {
    /// Get a reference to the connection pool
    fn pool(&self) -> &sqlx::PgPool;
}
