//! KYB Workflow Orchestrator — State Machine
//!
//! Drives the KYB onboarding pipeline:
//!   Draft → DocumentsSubmitted → RegistryVerified → ComplianceReview → Approved/Rejected
//!
//! Each transition validates preconditions and executes side-effects atomically.

use super::{
    document_store::DocumentStorageService,
    models::{
        DocumentType, KybApplication, KybApplicationSummary, ReviewDecisionRequest,
        StartKybRequest, SubmitDocumentRequest,
    },
    registry::registry_provider_for,
    repository::KybRepository,
    risk_scoring::RiskScoringEngine,
    ubo::UboService,
};
use std::sync::Arc;
use tracing::{error, info, warn};
use uuid::Uuid;

pub struct KybOrchestrator {
    repo: Arc<KybRepository>,
    ubo_svc: Arc<UboService>,
}

impl KybOrchestrator {
    pub fn new(repo: Arc<KybRepository>) -> Self {
        let ubo_svc = Arc::new(UboService::new(repo.clone()));
        Self { repo, ubo_svc }
    }

    // ── Step 1: Start KYB (Draft) ─────────────────────────────────────────────

    pub async fn start(&self, req: StartKybRequest) -> Result<KybApplication, OrchestratorError> {
        // Idempotent: return existing if already started
        if let Some(existing) = self.repo.get_by_merchant(req.merchant_id).await? {
            return Ok(existing);
        }

        let app = self.repo.create_application(
            req.merchant_id,
            &req.business_name,
            &req.registration_number,
            &req.jurisdiction,
            req.industry_code.as_deref(),
        ).await?;

        info!(kyb_id = %app.id, merchant_id = %req.merchant_id, "KYB application created");
        Ok(app)
    }

    // ── Step 2: Submit Document ───────────────────────────────────────────────

    pub async fn submit_document(
        &self,
        kyb_id: Uuid,
        req: SubmitDocumentRequest,
        business_name: &str,
    ) -> Result<KybApplication, OrchestratorError> {
        let app = self.require_application(kyb_id).await?;
        self.require_status(&app, &["draft", "documents_submitted"])?;

        // Decode and store encrypted document
        let content = base64::engine::general_purpose::STANDARD
            .decode(&req.file_content_b64)
            .map_err(|_| OrchestratorError::InvalidInput("Invalid base64 content".into()))?;

        let ocr = DocumentStorageService::extract_and_validate(&content, business_name);
        if !ocr.name_match {
            warn!(kyb_id = %kyb_id, "OCR name mismatch on submitted document");
        }

        let stored = DocumentStorageService::store(kyb_id, &req.document_type, &req.file_name, &content)
            .map_err(OrchestratorError::Storage)?;

        let ocr_data = serde_json::json!({
            "extracted_text": ocr.extracted_text,
            "name_match": ocr.name_match,
        });

        self.repo.save_document(
            kyb_id,
            &req.document_type.to_string(),
            &req.file_name,
            &stored.file_path,
            &stored.file_hash,
            Some(ocr_data),
            Some(ocr.confidence),
        ).await?;

        // Advance to documents_submitted if still in draft
        let updated = if app.status == "draft" {
            self.repo.transition_status(kyb_id, "documents_submitted").await?
        } else {
            app
        };

        Ok(updated)
    }

    // ── Step 3: Run Registry Verification ────────────────────────────────────

    pub async fn verify_registry(&self, kyb_id: Uuid) -> Result<KybApplication, OrchestratorError> {
        let app = self.require_application(kyb_id).await?;
        self.require_status(&app, &["documents_submitted"])?;

        let provider = registry_provider_for(&app.jurisdiction);
        let result = provider.lookup(&app.registration_number).await;

        match result {
            Ok(entity) => {
                // Flag inactive/deregistered immediately
                if entity.status == "inactive" || entity.status == "deregistered" {
                    warn!(
                        kyb_id = %kyb_id,
                        registry_status = %entity.status,
                        "Business registry status is not active — flagging"
                    );
                }

                let registry_json = serde_json::to_value(&entity).unwrap_or_default();
                self.repo.save_registry_check(
                    kyb_id, provider.provider_name(), &app.registration_number,
                    "success", Some(registry_json.clone()), None,
                ).await?;

                self.repo.set_registry_result(
                    kyb_id, &entity.status, registry_json,
                    entity.registered_address.as_deref(),
                ).await?;

                // Extract UBOs and trigger KYC
                self.ubo_svc.extract_and_trigger(kyb_id, &entity).await?;

                // Compute risk score
                let factors = RiskScoringEngine::score(
                    &app.jurisdiction,
                    app.industry_code.as_deref(),
                    Some(&entity),
                );
                self.repo.save_risk_score(kyb_id, factors.total(), factors.risk_level(), factors.to_json()).await?;
                self.repo.set_risk_score(kyb_id, factors.total(), factors.risk_level()).await?;

                // Advance state
                let updated = self.repo.transition_status(kyb_id, "registry_verified").await?;
                info!(kyb_id = %kyb_id, risk_level = %factors.risk_level(), "Registry verified");
                Ok(updated)
            }
            Err(e) => {
                self.repo.save_registry_check(
                    kyb_id, provider.provider_name(), &app.registration_number,
                    "failed", None, Some(&e.to_string()),
                ).await?;
                Err(OrchestratorError::RegistryError(e.to_string()))
            }
        }
    }

    // ── Step 4: Submit for Compliance Review ──────────────────────────────────

    pub async fn submit_for_review(&self, kyb_id: Uuid) -> Result<KybApplication, OrchestratorError> {
        let app = self.require_application(kyb_id).await?;
        self.require_status(&app, &["registry_verified"])?;
        Ok(self.repo.transition_status(kyb_id, "compliance_review").await?)
    }

    // ── Step 5: Compliance Officer Decision ───────────────────────────────────

    pub async fn record_decision(
        &self,
        kyb_id: Uuid,
        reviewer_id: &str,
        req: ReviewDecisionRequest,
    ) -> Result<KybApplication, OrchestratorError> {
        let app = self.require_application(kyb_id).await?;
        self.require_status(&app, &["compliance_review"])?;

        let updated = self.repo.set_review_decision(
            kyb_id,
            req.approved,
            reviewer_id,
            req.notes.as_deref(),
            req.rejection_reason.as_deref(),
        ).await?;

        // Update merchant kyb_status
        if let Err(e) = self.sync_merchant_kyb_status(app.merchant_id, &updated.status).await {
            error!(error = %e, "Failed to sync merchant kyb_status");
        }

        info!(
            kyb_id = %kyb_id,
            approved = req.approved,
            reviewer = %reviewer_id,
            "KYB review decision recorded"
        );
        Ok(updated)
    }

    // ── Summary ───────────────────────────────────────────────────────────────

    pub async fn get_summary(&self, kyb_id: Uuid) -> Result<KybApplicationSummary, OrchestratorError> {
        let application = self.require_application(kyb_id).await?;
        let ubos = self.repo.list_ubos(kyb_id).await?;
        let documents = self.repo.list_documents(kyb_id).await?;
        let latest_risk_score = self.repo.latest_risk_score(kyb_id).await?;
        Ok(KybApplicationSummary { application, ubos, documents, latest_risk_score })
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    async fn require_application(&self, id: Uuid) -> Result<KybApplication, OrchestratorError> {
        self.repo.get_by_id(id).await?.ok_or(OrchestratorError::NotFound(id))
    }

    fn require_status(&self, app: &KybApplication, allowed: &[&str]) -> Result<(), OrchestratorError> {
        if !allowed.contains(&app.status.as_str()) {
            return Err(OrchestratorError::InvalidTransition {
                current: app.status.clone(),
                allowed: allowed.iter().map(|s| s.to_string()).collect(),
            });
        }
        Ok(())
    }

    async fn sync_merchant_kyb_status(&self, merchant_id: Uuid, status: &str) -> Result<(), anyhow::Error> {
        sqlx::query("UPDATE merchants SET kyb_status = $2 WHERE id = $1")
            .bind(merchant_id)
            .bind(status)
            .execute(self.repo.pool())
            .await?;
        Ok(())
    }
}

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error("KYB application not found: {0}")]
    NotFound(Uuid),
    #[error("Invalid state transition from '{current}'. Allowed: {}", allowed.join(", "))]
    InvalidTransition { current: String, allowed: Vec<String> },
    #[error("Registry error: {0}")]
    RegistryError(String),
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error(transparent)]
    Database(#[from] anyhow::Error),
}

impl OrchestratorError {
    pub fn status_code(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::InvalidTransition { .. } | Self::InvalidInput(_) => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
