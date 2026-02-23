//! Conversion audit service
//! Handles quote creation, execution updates, and failure/expiry tracking.

use crate::database::conversion_audit_repository::{ConversionAudit, ConversionAuditRepository};
use crate::database::error::DatabaseError;
use crate::database::repository::Repository;
use sqlx::types::BigDecimal;
use uuid::Uuid;

/// Input for creating a conversion quote audit
#[derive(Debug, Clone)]
pub struct ConversionQuoteInput {
    pub user_id: Option<Uuid>,
    pub wallet_address: Option<String>,
    pub transaction_id: Option<Uuid>,
    pub from_currency: String,
    pub to_currency: String,
    pub from_amount: BigDecimal,
    pub to_amount: BigDecimal,
    pub rate: BigDecimal,
    pub fee_amount: BigDecimal,
    pub fee_currency: Option<String>,
    pub provider: Option<String>,
    pub metadata: serde_json::Value,
}

/// Service for conversion audit lifecycle
pub struct ConversionAuditService {
    repo: ConversionAuditRepository,
}

impl ConversionAuditService {
    pub fn new(repo: ConversionAuditRepository) -> Self {
        Self { repo }
    }

    /// Record a new conversion quote
    pub async fn create_quote(
        &self,
        input: ConversionQuoteInput,
    ) -> Result<ConversionAudit, DatabaseError> {
        self.repo
            .create(
                input.user_id,
                input.wallet_address.as_deref(),
                input.transaction_id,
                &input.from_currency,
                &input.to_currency,
                input.from_amount,
                input.to_amount,
                input.rate,
                input.fee_amount,
                input.fee_currency.as_deref(),
                input.provider.as_deref(),
                "quoted",
                None,
                input.metadata,
            )
            .await
    }

    /// Mark a conversion as executed
    pub async fn mark_executed(
        &self,
        audit_id: Uuid,
        transaction_id: Option<Uuid>,
    ) -> Result<ConversionAudit, DatabaseError> {
        if let Some(tx_id) = transaction_id {
            self.update_transaction_id(audit_id, tx_id).await?;
        }
        self.repo.update_status(audit_id, "executed", None).await
    }

    /// Mark a conversion as failed
    pub async fn mark_failed(
        &self,
        audit_id: Uuid,
        error_message: &str,
    ) -> Result<ConversionAudit, DatabaseError> {
        self.repo
            .update_status(audit_id, "failed", Some(error_message))
            .await
    }

    /// Mark a conversion as expired
    pub async fn mark_expired(&self, audit_id: Uuid) -> Result<ConversionAudit, DatabaseError> {
        self.repo.update_status(audit_id, "expired", None).await
    }

    /// Find a conversion audit by ID
    pub async fn find_by_id(
        &self,
        audit_id: Uuid,
    ) -> Result<Option<ConversionAudit>, DatabaseError> {
        self.repo.find_by_id(&audit_id.to_string()).await
    }

    /// Update a conversion audit record
    pub async fn update(&self, audit: &ConversionAudit) -> Result<ConversionAudit, DatabaseError> {
        self.repo.update(&audit.id.to_string(), audit).await
    }

    async fn update_transaction_id(
        &self,
        audit_id: Uuid,
        transaction_id: Uuid,
    ) -> Result<ConversionAudit, DatabaseError> {
        let mut audit = self
            .repo
            .find_by_id(&audit_id.to_string())
            .await?
            .ok_or_else(|| {
                DatabaseError::new(crate::database::error::DatabaseErrorKind::NotFound {
                    entity: "ConversionAudit".to_string(),
                    id: audit_id.to_string(),
                })
            })?;

        audit.transaction_id = Some(transaction_id);
        self.update(&audit).await
    }
}
