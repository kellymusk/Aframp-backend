//! HTTP handlers for the regulatory filing pipeline
//!
//! GET  /api/v1/regulatory-filings/:id           — get report
//! GET  /api/v1/regulatory-filings/:id/audit     — audit trail
//! POST /api/v1/regulatory-filings               — compile new report
//! POST /api/v1/regulatory-filings/:id/transmit  — transmit to agency
//! POST /api/v1/regulatory-filings/:id/ack       — record ACK
//! POST /api/v1/regulatory-filings/:id/nack      — record NACK
//! GET  /api/v1/regulatory-filings/gateways      — list agency gateways

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use super::{models::CreateReportRequest, service::RegulatoryFilingService};

pub type FilingState = Arc<RegulatoryFilingService>;

pub async fn create_report(
    State(svc): State<FilingState>,
    Json(req): Json<CreateReportRequest>,
) -> impl IntoResponse {
    let snapshot = serde_json::json!({ "compiled_at": chrono::Utc::now().to_rfc3339() });
    match svc.compile_report(req, snapshot).await {
        Ok(r) => (StatusCode::CREATED, Json(r)).into_response(),
        Err(e) => err(e),
    }
}

pub async fn get_report(
    State(svc): State<FilingState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match svc.get(id).await {
        Ok(Some(r)) => (StatusCode::OK, Json(r)).into_response(),
        Ok(None) => not_found(),
        Err(e) => err(e),
    }
}

pub async fn transmit_report(
    State(svc): State<FilingState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match svc.transmit(id).await {
        Ok(r) => (StatusCode::OK, Json(r)).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct AckBody {
    pub ack_code: String,
}

pub async fn record_ack(
    State(svc): State<FilingState>,
    Path(id): Path<Uuid>,
    Json(body): Json<AckBody>,
) -> impl IntoResponse {
    match svc.record_ack(id, &body.ack_code).await {
        Ok(r) => (StatusCode::OK, Json(r)).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct NackBody {
    pub nack_reason: String,
}

pub async fn record_nack(
    State(svc): State<FilingState>,
    Path(id): Path<Uuid>,
    Json(body): Json<NackBody>,
) -> impl IntoResponse {
    match svc.record_nack(id, &body.nack_reason).await {
        Ok(r) => (StatusCode::OK, Json(r)).into_response(),
        Err(e) => err(e),
    }
}

pub async fn get_audit(
    State(svc): State<FilingState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match svc.get_audit(id).await {
        Ok(events) => (StatusCode::OK, Json(serde_json::json!({ "events": events }))).into_response(),
        Err(e) => err(e),
    }
}

pub async fn list_gateways(State(svc): State<FilingState>) -> impl IntoResponse {
    match svc.get_gateways().await {
        Ok(gateways) => (StatusCode::OK, Json(serde_json::json!({ "gateways": gateways }))).into_response(),
        Err(e) => err(e),
    }
}

fn not_found() -> axum::response::Response {
    (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "not_found" }))).into_response()
}

fn err(e: anyhow::Error) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": e.to_string() })),
    )
        .into_response()
}
