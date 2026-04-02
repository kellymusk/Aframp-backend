//! HTTP handlers for the Nigeria → Kenya corridor API.

use crate::corridors::kenya::models::*;
use crate::corridors::kenya::service::{KenyaCorridorError, KenyaCorridorService};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

pub struct KenyaCorridorState {
    pub service: Arc<KenyaCorridorService>,
    pub pool: Arc<PgPool>,
}

// ── Generic wrappers ──────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub data: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Serialize)]
pub struct ApiError {
    pub success: bool,
    pub error: String,
    pub code: &'static str,
}

fn ok<T: Serialize>(data: T) -> Json<ApiResponse<T>> {
    Json(ApiResponse { success: true, data, message: None })
}

fn err_response(
    status: StatusCode,
    msg: String,
    code: &'static str,
) -> (StatusCode, Json<ApiError>) {
    (status, Json(ApiError { success: false, error: msg, code }))
}

fn map_corridor_error(e: KenyaCorridorError) -> (StatusCode, Json<ApiError>) {
    match &e {
        KenyaCorridorError::ComplianceDenied(_) => {
            err_response(StatusCode::FORBIDDEN, e.to_string(), "COMPLIANCE_DENIED")
        }
        KenyaCorridorError::RecipientInvalid(_) => {
            err_response(StatusCode::UNPROCESSABLE_ENTITY, e.to_string(), "RECIPIENT_INVALID")
        }
        KenyaCorridorError::LimitExceeded(_) => {
            err_response(StatusCode::UNPROCESSABLE_ENTITY, e.to_string(), "LIMIT_EXCEEDED")
        }
        KenyaCorridorError::CbkRequirement(_) => {
            err_response(StatusCode::UNPROCESSABLE_ENTITY, e.to_string(), "CBK_REQUIREMENT")
        }
        KenyaCorridorError::FxUnavailable(_) => {
            err_response(StatusCode::SERVICE_UNAVAILABLE, e.to_string(), "FX_UNAVAILABLE")
        }
        KenyaCorridorError::DisbursementFailed(_) => {
            err_response(StatusCode::BAD_GATEWAY, e.to_string(), "DISBURSEMENT_FAILED")
        }
        _ => err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
            "INTERNAL_ERROR",
        ),
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct QuoteQuery {
    pub cngn_amount: Decimal,
}

/// GET /api/corridors/kenya/quote?cngn_amount=1000
pub async fn get_quote_handler(
    State(state): State<Arc<KenyaCorridorState>>,
    Query(params): Query<QuoteQuery>,
) -> Result<Json<ApiResponse<KenyaTransferQuote>>, (StatusCode, Json<ApiError>)> {
    state
        .service
        .get_quote(params.cngn_amount)
        .await
        .map(ok)
        .map_err(map_corridor_error)
}

/// POST /api/corridors/kenya/transfer
pub async fn initiate_transfer_handler(
    State(state): State<Arc<KenyaCorridorState>>,
    Json(req): Json<KenyaTransferRequest>,
) -> Result<(StatusCode, Json<ApiResponse<KenyaTransferResponse>>), (StatusCode, Json<ApiError>)> {
    state
        .service
        .initiate_transfer(&req)
        .await
        .map(|r| {
            (
                StatusCode::CREATED,
                Json(ApiResponse {
                    success: true,
                    data: r,
                    message: Some("Transfer initiated".to_string()),
                }),
            )
        })
        .map_err(map_corridor_error)
}

/// GET /api/corridors/kenya/transfer/:id
pub async fn get_transfer_handler(
    State(state): State<Arc<KenyaCorridorState>>,
    Path(transfer_id): Path<Uuid>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiError>)> {
    let row = sqlx::query!(
        r#"
        SELECT transaction_id, status, from_amount, to_amount, metadata, created_at, updated_at
        FROM transactions
        WHERE transaction_id = $1 AND type = 'kenya_corridor'
        "#,
        transfer_id,
    )
    .fetch_optional(state.pool.as_ref())
    .await
    .map_err(|e| {
        err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
            "DATABASE_ERROR",
        )
    })?;

    match row {
        Some(r) => Ok(ok(serde_json::json!({
            "transfer_id": r.transaction_id,
            "status": r.status,
            "cngn_amount": r.from_amount,
            "kes_amount": r.to_amount,
            "metadata": r.metadata,
            "created_at": r.created_at,
            "updated_at": r.updated_at,
        }))),
        None => Err(err_response(
            StatusCode::NOT_FOUND,
            format!("Transfer {} not found", transfer_id),
            "NOT_FOUND",
        )),
    }
}
