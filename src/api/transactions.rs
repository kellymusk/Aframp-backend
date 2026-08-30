use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, ApiResult};
use crate::models::Payment;
use crate::services::payments;
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub before: Option<String>,
}

#[derive(Serialize)]
pub struct PaymentListResponse {
    pub payments: Vec<Payment>,
    pub next_cursor: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<PaymentListResponse>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request("no merchant associated with this account"))?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);

    let before = params
        .before
        .as_ref()
        .map(|s| s.parse::<DateTime<Utc>>())
        .transpose()
        .map_err(|_| bad_request("invalid before timestamp format"))?;

    let payments = payments::payments_by_merchant(&state.db, merchant_id, limit + 1, before)
        .await
        .map_err(internal)?;

    let next_cursor = if payments.len() > limit as usize {
        payments.get(limit as usize).map(|p| p.created_at.to_rfc3339())
    } else {
        None
    };

    let payments = payments.into_iter().take(limit as usize).collect();

    Ok(Json(PaymentListResponse {
        payments,
        next_cursor,
    }))
}