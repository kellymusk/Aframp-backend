use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_gateway, bad_request, bad_request_field, internal, ApiResult};
use crate::models::{CreateWithdrawalRequest, NewWithdrawal, Withdrawal};
use crate::pagination::{Cursor, Page};
use crate::services::withdrawals::{self, WithdrawalError};
use crate::validation::{is_valid_account_number, is_valid_bank_code};
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
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
) -> ApiResult<Json<Page<Withdrawal>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request("no merchant associated with this account"))?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(Cursor::decode(raw).ok_or_else(|| bad_request("invalid cursor"))?),
        None => None,
    };
    let withdrawals =
        withdrawals::withdrawals_by_merchant_cursor(&state.db, merchant_id, limit, cursor)
            .await
            .map_err(internal)?;
    Ok(Json(Page::new(withdrawals, limit, |w| Cursor {
        created_at: w.created_at,
        id: w.id,
    })))
}

fn map_withdrawal_error(err: WithdrawalError) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        WithdrawalError::InsufficientBalance => bad_request("insufficient available balance"),
        WithdrawalError::UnsupportedAsset => bad_request("withdrawals are only supported for the cNGN asset"),
        WithdrawalError::InvalidAmountPrecision => {
            bad_request("amount_stroops must be a whole number of kobo")
        }
        WithdrawalError::PayoutFailed(msg) => bad_gateway(&msg),
        WithdrawalError::Database(e) => internal(e),
    }
}