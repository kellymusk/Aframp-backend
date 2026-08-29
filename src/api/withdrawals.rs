use std::collections::BTreeMap;

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::auth::extractor::AuthUser;
use crate::error::{
    bad_gateway, bad_request, bad_request_with_fields, internal, unprocessable_entity, ApiResult,
};
use crate::models::{CreateWithdrawalRequest, NewWithdrawal, Withdrawal};
use crate::services::withdrawals::{self, WithdrawalError};
use crate::AppState;

fn is_valid_bank_code(bank_code: &str) -> bool {
    let bank_code = bank_code.trim();
    bank_code.len() == 3 && bank_code.chars().all(|ch| ch.is_ascii_digit())
}

fn is_valid_account_number(account_number: &str) -> bool {
    let account_number = account_number.trim();
    account_number.len() == 10 && account_number.chars().all(|ch| ch.is_ascii_digit())
}

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
    let mut field_errors = BTreeMap::new();
    if req.amount_stroops <= 0 {
        field_errors.insert("amount_stroops".into(), "must be positive".into());
    }
    if !is_valid_bank_code(&req.bank_code) {
        field_errors.insert("bank_code".into(), "must be exactly 3 digits".into());
    }
    if !is_valid_account_number(&req.account_number) {
        field_errors.insert("account_number".into(), "must be exactly 10 digits".into());
    }
    if !field_errors.is_empty() {
        return Err(bad_request_with_fields("validation failed", field_errors));
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
        .ok_or_else(|| bad_request("no merchant associated with this account"))?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let withdrawals = withdrawals::withdrawals_by_merchant(&state.db, merchant_id, limit)
        .await
        .map_err(internal)?;
    Ok(Json(withdrawals))
}

fn map_withdrawal_error(err: WithdrawalError) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        WithdrawalError::InsufficientBalance => bad_request("insufficient available balance"),
        WithdrawalError::UnsupportedAsset => bad_request("withdrawals are only supported for the cNGN asset"),
        WithdrawalError::InvalidAmountPrecision => {
            bad_request("amount_stroops must be a whole number of kobo")
        }
        WithdrawalError::AccountResolutionFailed(msg) => {
            unprocessable_entity(&msg)
        }
        WithdrawalError::PayoutFailed(msg) => bad_gateway(&msg),
        WithdrawalError::Database(e) => internal(e),
    }
}