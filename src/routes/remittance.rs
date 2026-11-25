use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct RemittanceRequest {
    pub destination: String,
    pub amount: String,
    pub asset: Option<String>,
    pub memo: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RemittanceResponse {
    pub id: String,
    pub status: String,
    pub destination: String,
    pub amount: String,
    pub asset: String,
    pub tx_hash: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct EstimateQuery {
    pub amount: String,
    pub asset: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EstimateResponse {
    pub amount: String,
    pub fee: String,
    pub total: String,
    pub asset: String,
}

fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}

fn parse_amount(raw: &str) -> Result<f64, Response> {
    let value: f64 = raw
        .trim()
        .parse()
        .map_err(|_| bad_request("invalid amount"))?;
    if !value.is_finite() || value <= 0.0 {
        return Err(bad_request("amount must be greater than zero"));
    }
    Ok(value)
}

fn estimate_fee(amount: f64) -> f64 {
    // Flat network fee plus a small percentage of the sent amount.
    let percentage = amount * 0.005;
    let fee = 0.01 + percentage;
    (fee * 100.0).round() / 100.0
}

pub async fn estimate(
    State(_state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<EstimateQuery>,
) -> Response {
    let amount = match parse_amount(&query.amount) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let asset = query.asset.unwrap_or_else(|| "XLM".to_string());
    let fee = estimate_fee(amount);
    let total = ((amount + fee) * 100.0).round() / 100.0;

    Json(EstimateResponse {
        amount: format!("{:.2}", amount),
        fee: format!("{:.2}", fee),
        total: format!("{:.2}", total),
        asset,
    })
    .into_response()
}

pub async fn send(
    State(_state): State<AppState>,
    Json(payload): Json<RemittanceRequest>,
) -> Response {
    if payload.destination.trim().is_empty() {
        return bad_request("destination is required");
    }
    let amount = match parse_amount(&payload.amount) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let asset = payload.asset.unwrap_or_else(|| "XLM".to_string());

    // The actual Stellar payment submission is handled by the payments service.
    // This endpoint validates the request and returns the accepted remittance.
    let id = format!("rem_{}", uuid::Uuid::new_v4().simple());
    let created_at = chrono::Utc::now().to_rfc3339();

    (
        StatusCode::ACCEPTED,
        Json(RemittanceResponse {
            id,
            status: "pending".to_string(),
            destination: payload.destination,
            amount: format!("{:.2}", amount),
            asset,
            tx_hash: None,
            created_at,
        }),
    )
        .into_response()
}

pub async fn history(State(_state): State<AppState>) -> Response {
    let remittances: Vec<RemittanceResponse> = Vec::new();
    Json(json!({ "remittances": remittances })).into_response()
}
