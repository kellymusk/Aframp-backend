//! Compliance Effectiveness Reporting — HTTP Handlers
//!
//! All endpoints require `compliance_officer` or `finance_director` role.
//! Audit events are logged for every report generation and download.

use super::{
    models::{GenerateReportRequest, ListReportsQuery},
    repository::ComplianceEffectivenessRepository,
    service::ReportGenerationService,
};
use crate::middleware::rbac::CallerIdentity;
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

// ── Shared State ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ComplianceEffectivenessState {
    pub service: Arc<ReportGenerationService>,
    pub repo: Arc<ComplianceEffectivenessRepository>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extract_ip(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
}

fn bad_request(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}

fn not_found(msg: &str) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": msg }))).into_response()
}

fn internal_error(msg: &str) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": msg }))).into_response()
}

// ── POST /compliance/reports — Generate a new report ─────────────────────────

pub async fn generate_report(
    State(state): State<Arc<ComplianceEffectivenessState>>,
    Extension(caller): Extension<CallerIdentity>,
    axum::extract::RawRequest(req): axum::extract::RawRequest,
    Json(body): Json<GenerateReportRequest>,
) -> Response {
    let (parts, _) = req.into_parts();
    let actor_ip = extract_ip(&parts.headers);

    if body.period_end <= body.period_start {
        return bad_request("period_end must be after period_start");
    }

    match state
        .service
        .generate(
            &body.report_type,
            body.period_start,
            body.period_end,
            &body.format,
            &caller.user_id,
        )
        .await
    {
        Ok(generated) => {
            // Log audit event
            let _ = state
                .repo
                .log_report_access(
                    generated.report.id,
                    "generated",
                    &caller.user_id,
                    &caller.role,
                    actor_ip.as_deref(),
                )
                .await;

            let content_type = generated.content_type;
            let filename = format!(
                "compliance_report_{}_{}.{}",
                generated.report.period_start.format("%Y%m%d"),
                generated.report.period_end.format("%Y%m%d"),
                generated.report.format,
            );

            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type),
                    (
                        header::CONTENT_DISPOSITION,
                        Box::leak(format!("attachment; filename=\"{filename}\"").into_boxed_str()),
                    ),
                ],
                generated.content,
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, actor = %caller.user_id, "Report generation failed");
            internal_error("Report generation failed")
        }
    }
}

// ── GET /compliance/reports — List reports ────────────────────────────────────

pub async fn list_reports(
    State(state): State<Arc<ComplianceEffectivenessState>>,
    Extension(_caller): Extension<CallerIdentity>,
    Query(query): Query<ListReportsQuery>,
) -> Response {
    match state.repo.list_reports(&query).await {
        Ok(page) => (StatusCode::OK, Json(page)).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to list compliance reports");
            internal_error("Failed to list reports")
        }
    }
}

// ── GET /compliance/reports/:id — Get report metadata ────────────────────────

pub async fn get_report(
    State(state): State<Arc<ComplianceEffectivenessState>>,
    Extension(caller): Extension<CallerIdentity>,
    axum::extract::RawRequest(req): axum::extract::RawRequest,
    Path(report_id): Path<Uuid>,
) -> Response {
    let (parts, _) = req.into_parts();
    let actor_ip = extract_ip(&parts.headers);

    match state.repo.get_report(report_id).await {
        Ok(Some(report)) => {
            let _ = state
                .repo
                .log_report_access(
                    report_id,
                    "downloaded",
                    &caller.user_id,
                    &caller.role,
                    actor_ip.as_deref(),
                )
                .await;
            (StatusCode::OK, Json(report)).into_response()
        }
        Ok(None) => not_found("Report not found"),
        Err(e) => {
            tracing::error!(error = %e, report_id = %report_id, "Failed to fetch report");
            internal_error("Failed to fetch report")
        }
    }
}
