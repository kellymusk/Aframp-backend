use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

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

#[derive(serde::Serialize)]
pub struct WithdrawalView {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub amount_stroops: i64,
    pub asset: String,
    pub status: String,
    pub provider: Option<String>,
    pub provider_reference: Option<String>,
    pub bank_code: Option<String>,
    pub account_number: Option<String>,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateWithdrawalRequest>,
) -> ApiResult<Json<WithdrawalView>> {
    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })?;
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
    Ok(Json(to_view(&withdrawal)))
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<WithdrawalView>>> {
    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(
            Cursor::decode(raw)
                .ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?,
        ),
        None => None,
    };
    let withdrawals =
        withdrawals::withdrawals_by_merchant_cursor(&state.db, merchant_id, limit, cursor)
            .await
            .map_err(internal)?;
    let views: Vec<WithdrawalView> = withdrawals.iter().map(to_view).collect();
    Ok(Json(Page::new(views, limit, |w| Cursor {
        created_at: w.created_at,
        id: w.id,
    })))
}

fn to_view(withdrawal: &Withdrawal) -> WithdrawalView {
    WithdrawalView {
        id: withdrawal.id,
        merchant_id: withdrawal.merchant_id,
        amount_stroops: withdrawal.amount_stroops,
        asset: withdrawal.asset.clone(),
        status: withdrawal.status.clone(),
        provider: withdrawal.provider.clone(),
        provider_reference: withdrawal.provider_reference.clone(),
        bank_code: withdrawal.bank_code.clone(),
        account_number: withdrawal
            .account_number
            .as_deref()
            .map(mask_account_number),
        failure_reason: withdrawal.failure_reason.clone(),
        created_at: withdrawal.created_at,
        updated_at: withdrawal.updated_at,
    }
}

fn mask_account_number(account_number: &str) -> String {
    let last4_start = account_number.len().saturating_sub(4);
    let last4 = &account_number[last4_start..];
    format!("****{last4}")
}

fn map_withdrawal_error(
    err: WithdrawalError,
) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        WithdrawalError::InsufficientBalance => bad_request(
            ErrorCode::InsufficientBalance,
            "insufficient available balance",
        ),
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
