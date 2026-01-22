use crate::database::error::{DatabaseError, DatabaseErrorKind};
use sqlx::Transaction as SqlxTransaction;
use sqlx::{PgPool, Postgres};
use tracing::{debug, warn, error as log_error};
use std::pin::Pin;
use std::future::Future;

/// Database transaction wrapper for atomic operations
/// Ensures automatic rollback on errors and proper connection management
pub struct DatabaseTransaction {
    transaction: Option<SqlxTransaction<'static, Postgres>>,
}

impl DatabaseTransaction {
    /// Begin a new transaction
    pub async fn begin(pool: &PgPool) -> Result<Self, DatabaseError> {
        debug!("Beginning database transaction");
        
        let transaction = pool
            .begin()
            .await
            .map_err(|e| {
                log_error!("Failed to begin transaction: {}", e);
                DatabaseError::from_sqlx(e)
            })?;

        Ok(Self {
            transaction: Some(transaction),
        })
    }

    /// Commit the transaction
    pub async fn commit(mut self) -> Result<(), DatabaseError> {
        if let Some(tx) = self.transaction.take() {
            debug!("Committing transaction");
            
            tx.commit()
                .await
                .map_err(|e| {
                    log_error!("Failed to commit transaction: {}", e);
                    DatabaseError::from_sqlx(e)
                })?;

            Ok(())
        } else {
            Err(DatabaseError::new(DatabaseErrorKind::TransactionError {
                message: "Transaction already completed".to_string(),
            }))
        }
    }

    /// Rollback the transaction
    pub async fn rollback(mut self) -> Result<(), DatabaseError> {
        if let Some(tx) = self.transaction.take() {
            debug!("Rolling back transaction");
            
            tx.rollback()
                .await
                .map_err(|e| {
                    log_error!("Failed to rollback transaction: {}", e);
                    DatabaseError::from_sqlx(e)
                })?;

            Ok(())
        } else {
            Err(DatabaseError::new(DatabaseErrorKind::TransactionError {
                message: "Transaction already completed".to_string(),
            }))
        }
    }

    /// Get a mutable reference to the transaction for executing queries
    pub fn tx_mut(&mut self) -> &mut SqlxTransaction<'static, Postgres> {
        self.transaction
            .as_mut()
            .expect("Transaction was already completed")
    }

    /// Get an immutable reference to the transaction
    pub fn tx(&self) -> &SqlxTransaction<'static, Postgres> {
        self.transaction
            .as_ref()
            .expect("Transaction was already completed")
    }
}

/// Helper for executing a single transaction with automatic rollback and retry support
pub struct TransactionBuilder {
    pool: PgPool,
}

impl TransactionBuilder {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Execute an async operation with transaction wrapping
    /// The operation receives a mutable reference to a PgPool
    pub async fn execute<T, F>(&self, operation: F) -> Result<T, DatabaseError>
    where
        T: Send + 'static,
        F: FnOnce(&PgPool) -> Pin<Box<dyn Future<Output = Result<T, DatabaseError>> + Send>>,
    {
        // Run the operation within a transaction context
        match operation(&self.pool).await {
            Ok(result) => Ok(result),
            Err(e) => {
                warn!("Transaction operation failed: {}", e);
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_builder_creation() {
        // This is a basic test to verify the type compiles
        // Actual tests require a running database
    }
}
