use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, bad_request_field, internal, ApiResult, ErrorCode};
use crate::models::{CreateWithdrawalRequest, ListParams, NewWithdrawal, Withdrawal};
use crate::pagination::{Cursor, Page};
use crate::services::withdrawals;
use crate::validation::{is_valid_account_number, is_valid_bank_code};
use crate::AppState;

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
    let resolved = withdrawals::verify_bank_account(
        state.payment_provider.as_ref(),
        &params.bank_code,
        &params.account_number,
    )
    .await?;
    Ok(Json(VerifiedAccount {
        account_name: resolved.account_name,
        bank_code: params.bank_code,
        account_number: params.account_number,
    }))
}

#[derive(Deserialize)]
pub struct WithdrawalFeeParams {
    pub amount_stroops: i64,
}

#[derive(serde::Serialize)]
pub struct WithdrawalFeeResponse {
    pub amount_stroops: i64,
    pub fee_stroops: i64,
    pub net_amount_stroops: i64,
}

pub async fn withdrawal_fee(
    Query(params): Query<WithdrawalFeeParams>,
) -> ApiResult<Json<WithdrawalFeeResponse>> {
    if params.amount_stroops <= 0 {
        return Err(bad_request_field(
            "amount_stroops",
            "must be a positive number",
        ));
    }
    
    let (fee_stroops, net_amount_stroops) = withdrawals::calculate_withdrawal_fee(params.amount_stroops);
    
    Ok(Json(WithdrawalFeeResponse {
        amount_stroops: params.amount_stroops,
        fee_stroops,
        net_amount_stroops,
    }))
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<Json<WithdrawalView>> {
    let req = CreateWithdrawalRequest::from_json(&body)
        .map_err(|(field, msg)| bad_request_field(field, msg))?;

    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })?;
    // Refuse if the merchant is suspended.
    let merchant = crate::services::users::merchant_by_id(&state.db, merchant_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "merchant not found"))?;
    if merchant.is_suspended() {
        return Err(crate::error::forbidden(
            ErrorCode::Forbidden,
            "this merchant account has been suspended",
        ));
    }

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
    let withdrawal = withdrawals::create_withdrawal_idempotent(
        &state.db,
        state.payment_provider.as_ref(),
        NewWithdrawal {
            merchant_id,
            amount_stroops: req.amount_stroops,
            asset: req.asset.unwrap_or_else(|| "cNGN".into()),
            bank_code: req.bank_code,
            account_number: req.account_number,
            idempotency_key: idempotency_key.clone(),
        },
        idempotency_key.as_deref(),
        state.daily_withdrawal_limit_stroops,
    )
    .await?;
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
    let limit = params.merchant_limit();
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
        status: withdrawal.status.to_string(),
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
