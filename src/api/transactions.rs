use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, ApiResult, ErrorCode};
use crate::models::Payment;
use crate::services::payments;
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<Payment>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let payments = payments::payments_by_merchant(&state.db, merchant_id, limit)
        .await
        .map_err(internal)?;
    Ok(Json(payments))
}