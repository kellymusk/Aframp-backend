use axum::extract::{Path, Query, State};
use axum::response::Html;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::extractor::AdminUser;
use crate::error::{internal, not_found, ApiResult, ErrorCode};
use crate::models::{
    AdminMerchantRow, AdminOverview, AdminPaymentRequestRow, AdminTransactionRow, AdminUserRow,
    AdminWalletRow, AdminWithdrawalRow, Merchant,
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

/// `POST /admin/merchants/{id}/suspend` — suspend a merchant account.
/// The merchant will receive 403 on /login, /payment-requests, and /withdraw
/// until unsuspended.
pub async fn suspend_merchant(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Merchant>> {
    let merchant = admin::suspend_merchant(&state.db, id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::MerchantNotFound, "merchant not found"))?;
    Ok(Json(merchant))
}

/// `POST /admin/merchants/{id}/unsuspend` — reinstate a suspended merchant.
pub async fn unsuspend_merchant(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Merchant>> {
    let merchant = admin::unsuspend_merchant(&state.db, id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::MerchantNotFound, "merchant not found"))?;
    Ok(Json(merchant))
}

/// `POST /admin/users/{id}/unlock` — clear an account lockout applied by the
/// failed-login counter. Also resets `failed_login_count` to 0.
pub async fn unlock_user(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<axum::http::StatusCode> {
    let found = admin::unlock_user(&state.db, id)
        .await
        .map_err(internal)?;
    if found {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(not_found(ErrorCode::UserNotFound, "user not found"))
    }
}

/// Static dashboard shell. Unauthenticated by design — it's markup and JS with
/// no data baked in. It logs in through the normal `/login` endpoint (setting
/// the same HttpOnly session cookie a browser client would get) and every data
/// fetch after that rides the cookie same-origin; nothing here bypasses the
/// `AdminUser` check on the JSON routes below.
pub async fn dashboard() -> Html<&'static str> {
    Html(include_str!("admin_dashboard.html"))
}
