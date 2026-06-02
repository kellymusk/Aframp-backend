//! HTTP handlers for the treasury bonds module
//!
//! GET  /api/v1/treasury-bonds/instruments            — list active instruments
//! POST /api/v1/treasury-bonds/instruments            — register instrument
//! GET  /api/v1/treasury-bonds/instruments/:id        — get instrument
//! POST /api/v1/treasury-bonds/allocations            — allocate bond units
//! GET  /api/v1/treasury-bonds/allocations/:id        — get allocation
//! POST /api/v1/treasury-bonds/allocations/:id/liquidate — liquidate
//! POST /api/v1/treasury-bonds/sweep-policy           — upsert sweep policy
//! GET  /api/v1/treasury-bonds/sweep-policy/:tenant_id — get sweep policy

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use uuid::Uuid;

use super::{
    models::{AllocateBondRequest, RegisterBondRequest, SweepPolicyRequest},
    service::TreasuryBondsService,
};

pub type BondsState = Arc<TreasuryBondsService>;

pub async fn list_instruments(State(svc): State<BondsState>) -> impl IntoResponse {
    match svc.list_instruments().await {
        Ok(instruments) => (StatusCode::OK, Json(serde_json::json!({ "instruments": instruments }))).into_response(),
        Err(e) => err(e),
    }
}

pub async fn register_instrument(
    State(svc): State<BondsState>,
    Json(req): Json<RegisterBondRequest>,
) -> impl IntoResponse {
    match svc.register_instrument(req).await {
        Ok(inst) => (StatusCode::CREATED, Json(inst)).into_response(),
        Err(e) => err(e),
    }
}

pub async fn get_instrument(
    State(svc): State<BondsState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match svc.get_instrument(id).await {
        Ok(Some(inst)) => (StatusCode::OK, Json(inst)).into_response(),
        Ok(None) => not_found(),
        Err(e) => err(e),
    }
}

pub async fn create_allocation(
    State(svc): State<BondsState>,
    Json(req): Json<AllocateBondRequest>,
) -> impl IntoResponse {
    match svc.allocate(req).await {
        Ok(alloc) => (StatusCode::CREATED, Json(alloc)).into_response(),
        Err(e) => err(e),
    }
}

pub async fn get_allocation(
    State(svc): State<BondsState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match svc.get_allocation(id).await {
        Ok(Some(alloc)) => (StatusCode::OK, Json(alloc)).into_response(),
        Ok(None) => not_found(),
        Err(e) => err(e),
    }
}

pub async fn liquidate_allocation(
    State(svc): State<BondsState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match svc.liquidate(id).await {
        Ok(alloc) => (StatusCode::OK, Json(alloc)).into_response(),
        Err(e) => err(e),
    }
}

pub async fn upsert_sweep_policy(
    State(svc): State<BondsState>,
    Json(req): Json<SweepPolicyRequest>,
) -> impl IntoResponse {
    match svc.upsert_sweep_policy(req).await {
        Ok(policy) => (StatusCode::OK, Json(policy)).into_response(),
        Err(e) => err(e),
    }
}

pub async fn get_sweep_policy(
    State(svc): State<BondsState>,
    Path(tenant_id): Path<Uuid>,
) -> impl IntoResponse {
    match svc.get_sweep_policy(tenant_id).await {
        Ok(Some(policy)) => (StatusCode::OK, Json(policy)).into_response(),
        Ok(None) => not_found(),
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
