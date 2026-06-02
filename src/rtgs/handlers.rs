//! HTTP handlers for the RTGS interbank settlement rail
//!
//! GET  /api/v1/rtgs/pools                        — list settlement pools
//! POST /api/v1/rtgs/pools                        — register pool
//! POST /api/v1/rtgs/settlements/prepare          — 2PC prepare
//! POST /api/v1/rtgs/settlements/:id/commit       — 2PC commit
//! POST /api/v1/rtgs/settlements/:id/reverse      — abort/reverse
//! POST /api/v1/rtgs/settlements/:id/hold         — hold for reconciliation
//! GET  /api/v1/rtgs/settlements/:id              — get entry
//! GET  /api/v1/rtgs/settlements/:id/logs         — reconciliation logs

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use uuid::Uuid;

use super::{
    models::{CommitSettlementRequest, CreateSettlementRequest, RegisterPoolRequest, ReverseSettlementRequest},
    service::RtgsService,
};

pub type RtgsState = Arc<RtgsService>;

pub async fn list_pools(State(svc): State<RtgsState>) -> impl IntoResponse {
    match svc.list_pools().await {
        Ok(pools) => (StatusCode::OK, Json(serde_json::json!({ "pools": pools }))).into_response(),
        Err(e) => err(e),
    }
}

pub async fn register_pool(
    State(svc): State<RtgsState>,
    Json(req): Json<RegisterPoolRequest>,
) -> impl IntoResponse {
    match svc.register_pool(req).await {
        Ok(pool) => (StatusCode::CREATED, Json(pool)).into_response(),
        Err(e) => err(e),
    }
}

pub async fn prepare_settlement(
    State(svc): State<RtgsState>,
    Json(req): Json<CreateSettlementRequest>,
) -> impl IntoResponse {
    match svc.prepare_settlement(req).await {
        Ok(entry) => (StatusCode::CREATED, Json(entry)).into_response(),
        Err(e) => err(e),
    }
}

pub async fn commit_settlement(
    State(svc): State<RtgsState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CommitSettlementRequest>,
) -> impl IntoResponse {
    match svc.commit_settlement(id, req).await {
        Ok(entry) => (StatusCode::OK, Json(entry)).into_response(),
        Err(e) => err(e),
    }
}

pub async fn reverse_settlement(
    State(svc): State<RtgsState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ReverseSettlementRequest>,
) -> impl IntoResponse {
    match svc.reverse_settlement(id, &body.reason).await {
        Ok(entry) => (StatusCode::OK, Json(entry)).into_response(),
        Err(e) => err(e),
    }
}

pub async fn hold_settlement(
    State(svc): State<RtgsState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match svc.hold_for_reconciliation(id).await {
        Ok(entry) => (StatusCode::OK, Json(entry)).into_response(),
        Err(e) => err(e),
    }
}

pub async fn get_entry(
    State(svc): State<RtgsState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match svc.get_entry(id).await {
        Ok(Some(entry)) => (StatusCode::OK, Json(entry)).into_response(),
        Ok(None) => not_found(),
        Err(e) => err(e),
    }
}

pub async fn get_logs(
    State(svc): State<RtgsState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match svc.get_reconciliation_logs(id).await {
        Ok(logs) => (StatusCode::OK, Json(serde_json::json!({ "logs": logs }))).into_response(),
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
