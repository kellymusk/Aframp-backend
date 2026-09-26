use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth::extractor::AuthUser;
use crate::error::{bad_gateway, bad_request, bad_request_field, internal, ApiResult, ErrorCode};
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

#[derive(Deserialize)]
pub struct VerifyBankParams {
    pub bank_code: String,
    pub account_number: String,
}

#[derive(Serialize)]
pub struct VerifiedAccount {
    pub account_name: String,
    pub bank_code: String,
    pub account_number: String,
}

/// Resolve a bank account (account number + bank code) to its registered
/// account name before any withdrawal is attempted. This lets merchants
/// confirm the recipient details up front instead of discovering a bad
/// account only after a transfer has been attempted.
pub async fn verify_bank(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<VerifyBankParams>,
) -> ApiResult<Json<VerifiedAccount>> {
    if !is_valid_bank_code(&params.bank_code) {
        return Err(bad_request_field("bank_code", "must be a 3-digit code"));
    }
    if !is_valid_account_number(&params.account_number) {
        return Err(bad_request_field(
            "account_number",
            "must be a 10-digit NUBAN account number",
        ));
    }
    let resolved = withdrawals::resolve_account(
        state.payment_provider.as_ref(),
        &params.bank_code,
        &params.account_number,
    )
    .await
    .map_err(map_withdrawal_error)?;
    Ok(Json(VerifiedAccount {
        account_name: resolved.account_name,
        bank_code: params.bank_code,
        account_number: params.account_number,
    }))
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<CreateWithdrawalRequest>,
) -> ApiResult<Json<Withdrawal>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
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
    let idempotency_key = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_owned);
    let withdrawal = withdrawals::create_withdrawal(
        &state.db,
        state.payment_provider.as_ref(),
        NewWithdrawal {
            merchant_id,
            amount_stroops: req.amount_stroops,
            asset: req.asset.unwrap_or_else(|| "cNGN".into()),
            bank_code: req.bank_code,
            account_number: req.account_number,
            idempotency_key,
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
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(Cursor::decode(raw).ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?),
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
        WithdrawalError::AccountResolutionFailed(msg) => {
            bad_request(ErrorCode::AccountResolutionFailed, &msg)
        }
        WithdrawalError::PayoutFailed(msg) => bad_gateway(ErrorCode::PayoutFailed, &msg),
        WithdrawalError::Database(e) => internal(e),
    }
}