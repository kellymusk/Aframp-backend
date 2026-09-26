use axum::extract::{Path, Query, State};
use axum::response::Html;
use axum::Json;
use serde::Deserialize;

use crate::auth::extractor::AdminUser;
use crate::error::{internal, ApiResult};
use crate::models::{
    AdminMerchantRow, AdminOverview, AdminPaymentRequestRow, AdminTransactionRow, AdminUserRow,
    AdminWalletRow, AdminWithdrawalRow,
};
use crate::services::admin;
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
}

fn limit(params: &ListParams) -> i64 {
    params.limit.unwrap_or(100).clamp(1, 500)
}

pub async fn overview(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> ApiResult<Json<AdminOverview>> {
    let overview = admin::overview(&state.db).await.map_err(internal)?;
    Ok(Json(overview))
}

pub async fn users(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminUserRow>>> {
    let rows = admin::users(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

/// Manually clear a lockout on a user account. Resets the failed-login counter
/// and clears `locked_until` so the user can attempt to log in again.
pub async fn unlock_user(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(user_id): Path<i64>,
) -> ApiResult<Json<AdminUserRow>> {
    let row = admin::unlock_user(&state.db, user_id).await.map_err(internal)?;
    Ok(Json(row))
}

pub async fn merchants(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminMerchantRow>>> {
    let rows = admin::merchants(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn wallets(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminWalletRow>>> {
    let rows = admin::wallets(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn transactions(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminTransactionRow>>> {
    let rows = admin::transactions(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn withdrawals(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminWithdrawalRow>>> {
    let rows = admin::withdrawals(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn payment_requests(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminPaymentRequestRow>>> {
    let rows = admin::payment_requests(&state.db, limit(&params))
        .await
        .map_err(internal)?;
    Ok(Json(rows))
}

/// Static dashboard shell. Unauthenticated by design — it's markup and JS with
/// no data baked in. It logs in through the normal `/login` endpoint (setting
/// the same HttpOnly session cookie a browser client would get) and every data
/// fetch after that rides the cookie same-origin; nothing here bypasses the
/// `AdminUser` check on the JSON routes below.
pub async fn dashboard() -> Html<&'static str> {
    Html(include_str!("admin_dashboard.html"))
}
