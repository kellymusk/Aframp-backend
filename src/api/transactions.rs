use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, ApiResult};
use crate::models::Payment;
use crate::pagination::{Cursor, Page};
use crate::services::payments;
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<Payment>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request("no merchant associated with this account"))?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(Cursor::decode(raw).ok_or_else(|| bad_request("invalid cursor"))?),
        None => None,
    };
    let payments = payments::payments_by_merchant_cursor(&state.db, merchant_id, limit, cursor)
        .await
        .map_err(internal)?;
    Ok(Json(Page::new(payments, limit, |p| Cursor {
        created_at: p.created_at,
        id: p.id,
    })))
}