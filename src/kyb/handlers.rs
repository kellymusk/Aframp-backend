//! KYB API Handlers
//!
//! Merchant-facing: start KYB, submit documents.
//! Compliance-facing: trigger registry check, submit for review, record decision.

use super::{
    models::{ReviewDecisionRequest, StartKybRequest, SubmitDocumentRequest},
    orchestrator::{KybOrchestrator, OrchestratorError},
};
use crate::middleware::rbac::CallerIdentity;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct KybState {
    pub orchestrator: Arc<KybOrchestrator>,
}

fn err(e: OrchestratorError) -> Response {
    let status = e.status_code();
    (status, Json(json!({ "error": e.to_string() }))).into_response()
}

// POST /kyb/applications
pub async fn start_kyb(
    State(s): State<Arc<KybState>>,
    Json(body): Json<StartKybRequest>,
) -> Response {
    match s.orchestrator.start(body).await {
        Ok(app) => (StatusCode::CREATED, Json(app)).into_response(),
        Err(e) => err(e),
    }
}

// POST /kyb/applications/:id/documents
pub async fn submit_document(
    State(s): State<Arc<KybState>>,
    Path(kyb_id): Path<Uuid>,
    Json(body): Json<SubmitDocumentRequest>,
) -> Response {
    // Fetch business name for OCR validation
    let business_name = match s.orchestrator.get_summary(kyb_id).await {
        Ok(summary) => summary.application.business_name,
        Err(e) => return err(e),
    };
    match s.orchestrator.submit_document(kyb_id, body, &business_name).await {
        Ok(app) => (StatusCode::OK, Json(app)).into_response(),
        Err(e) => err(e),
    }
}

// POST /kyb/applications/:id/verify-registry  [compliance_officer]
pub async fn verify_registry(
    State(s): State<Arc<KybState>>,
    Extension(_caller): Extension<CallerIdentity>,
    Path(kyb_id): Path<Uuid>,
) -> Response {
    match s.orchestrator.verify_registry(kyb_id).await {
        Ok(app) => (StatusCode::OK, Json(app)).into_response(),
        Err(e) => err(e),
    }
}

// POST /kyb/applications/:id/submit-review  [compliance_officer]
pub async fn submit_for_review(
    State(s): State<Arc<KybState>>,
    Extension(_caller): Extension<CallerIdentity>,
    Path(kyb_id): Path<Uuid>,
) -> Response {
    match s.orchestrator.submit_for_review(kyb_id).await {
        Ok(app) => (StatusCode::OK, Json(app)).into_response(),
        Err(e) => err(e),
    }
}

// POST /kyb/applications/:id/decision  [compliance_officer]
pub async fn record_decision(
    State(s): State<Arc<KybState>>,
    Extension(caller): Extension<CallerIdentity>,
    Path(kyb_id): Path<Uuid>,
    Json(body): Json<ReviewDecisionRequest>,
) -> Response {
    match s.orchestrator.record_decision(kyb_id, &caller.user_id, body).await {
        Ok(app) => (StatusCode::OK, Json(app)).into_response(),
        Err(e) => err(e),
    }
}

// GET /kyb/applications/:id
pub async fn get_application(
    State(s): State<Arc<KybState>>,
    Path(kyb_id): Path<Uuid>,
) -> Response {
    match s.orchestrator.get_summary(kyb_id).await {
        Ok(summary) => (StatusCode::OK, Json(summary)).into_response(),
        Err(e) => err(e),
    }
}
