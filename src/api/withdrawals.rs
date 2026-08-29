use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_gateway, bad_request, bad_request_field, internal, ApiResult};
use crate::models::{CreateWithdrawalRequest, NewWithdrawal, Withdrawal};
use crate::services::withdrawals::{self, WithdrawalError};
use crate::validation::{is_valid_account_number, is_valid_bank_code};
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateWithdrawalRequest>,
) -> ApiResult<Json<Withdrawal>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request("no merchant associated with this account"))?;
    if req.amount_stroops <= 0 {
        return Err(bad_request_field(
            "amount_stroops",
            "must be a positive number",
        ));
    }
    if !is_valid_bank_code(&req.bank_code) {
        return Err(bad_request_field("bank_code", "must be a 3-digit code"));
    }
    if !is_valid_account_number(&req.account_number) {
        return Err(bad_request_field(
            "account_number",
            "must be a 10-digit NUBAN account number",
        ));
    }
    let withdrawal = withdrawals::create_withdrawal(
        &state.db,
        state.payment_provider.as_ref(),
        NewWithdrawal {
            merchant_id,
            amount_stroops: req.amount_stroops,
            asset: req.asset.unwrap_or_else(|| "cNGN".into()),
            bank_code: req.bank_code,
            account_number: req.account_number,
        },
    )
    .await
    .map_err(map_withdrawal_error)?;
    Ok(Json(withdrawal))
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<Withdrawal>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let withdrawals = withdrawals::withdrawals_by_merchant(&state.db, merchant_id, limit)
        .await
        .map_err(internal)?;
    Ok(Json(withdrawals))
}

fn map_withdrawal_error(err: WithdrawalError) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        WithdrawalError::InsufficientBalance => {
            bad_request(ErrorCode::InsufficientBalance, "insufficient available balance")
        }
        WithdrawalError::UnsupportedAsset => bad_request(
            ErrorCode::UnsupportedAsset,
            "withdrawals are only supported for the cNGN asset",
        ),
        WithdrawalError::InvalidAmountPrecision => bad_request(
            ErrorCode::InvalidAmount,
            "amount_stroops must be a whole number of kobo",
        ),
        WithdrawalError::PayoutFailed(msg) => bad_gateway(ErrorCode::PayoutFailed, &msg),
        WithdrawalError::Database(e) => internal(e),
    }
}