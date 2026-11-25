use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::state::AppState;

/// Request body for `POST /remittance`.
#[derive(Debug, Deserialize)]
pub struct SendRemittanceRequest {
    /// Destination Stellar account (G...).
    pub destination: String,
    /// Amount to send, in the asset's units.
    pub amount: String,
    /// Optional asset code; defaults to XLM.
    #[serde(default)]
    pub asset: Option<String>,
    /// Optional memo attached to the payment.
    #[serde(default)]
    pub memo: Option<String>,
}

/// A single remittance record returned by the API.
#[derive(Debug, Serialize)]
pub struct Remittance {
    pub id: String,
    pub destination: String,
    pub amount: String,
    pub asset: String,
    pub memo: Option<String>,
    pub status: String,
    pub tx_hash: Option<String>,
    pub created_at: String,
}

/// Query params for `GET /remittances`.
#[derive(Debug, Deserialize)]
pub struct ListRemittancesQuery {
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
}

/// Query params for `GET /remittance/estimate`.
#[derive(Debug, Deserialize)]
pub struct EstimateQuery {
    pub amount: String,
    #[serde(default)]
    pub asset: Option<String>,
}

/// Response for `GET /remittance/estimate`.
#[derive(Debug, Serialize)]
pub struct EstimateResponse {
    pub amount: String,
    pub asset: String,
    pub fee: String,
    pub total: String,
}

/// `POST /remittance` — send a Stellar payment from the merchant wallet.
pub async fn send_remittance(
    State(state): State<AppState>,
    Json(payload): Json<SendRemittanceRequest>,
) -> impl IntoResponse {
    let amount: f64 = match payload.amount.parse() {
        Ok(v) if v > 0.0 => v,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid amount" })),
            )
                .into_response();
        }
    };

    if payload.destination.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "destination is required" })),
        )
            .into_response();
    }

    let asset = payload.asset.unwrap_or_else(|| "XLM".to_string());

    match state
        .stellar
        .send_payment(&payload.destination, &amount.to_string(), &asset, payload.memo.as_deref())
        .await
    {
        Ok(tx_hash) => {
            let record = Remittance {
                id: Uuid::new_v4().to_string(),
                destination: payload.destination,
                amount: amount.to_string(),
                asset,
                memo: payload.memo,
                status: "submitted".to_string(),
                tx_hash: Some(tx_hash),
                created_at: chrono::Utc::now().to_rfc3339(),
            };
            (StatusCode::CREATED, Json(record)).into_response()
        }
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /remittances` — return remittance history for the merchant.
pub async fn list_remittances(
    State(state): State<AppState>,
    Query(query): Query<ListRemittancesQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(50).min(200);
    let offset = query.offset.unwrap_or(0);

    match state.remittances.list(limit, offset).await {
        Ok(items) => (StatusCode::OK, Json(json!({ "data": items }))).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /remittance/estimate` — return a fee estimate for a transfer.
pub async fn estimate_remittance(
    State(state): State<AppState>,
    Query(query): Query<EstimateQuery>,
) -> impl IntoResponse {
    let amount: f64 = match query.amount.parse() {
        Ok(v) if v > 0.0 => v,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid amount" })),
            )
                .into_response();
        }
    };

    let asset = query.asset.unwrap_or_else(|| "XLM".to_string());

    match state.stellar.estimate_fee(&amount.to_string(), &asset).await {
        Ok(fee) => {
            let total = amount + fee;
            (
                StatusCode::OK,
                Json(EstimateResponse {
                    amount: amount.to_string(),
                    asset,
                    fee: fee.to_string(),
                    total: total.to_string(),
                }),
            )
                .into_response()
        }
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}
