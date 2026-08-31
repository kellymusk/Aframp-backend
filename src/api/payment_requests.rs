use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, not_found, ApiResult};
use crate::models::{CreatePaymentRequestRequest, PaymentRequest};
use crate::services::{payment_requests, wallets};
use crate::AppState;

#[derive(Serialize)]
pub struct PaymentRequestView {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub address: String,
    pub network: String,
    pub amount_stroops: i64,
    pub asset: String,
    pub memo: String,
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    /// SEP-0007 payment URI a Stellar wallet can open directly to pay this
    /// request. `None` for credit assets we don't have a real issuer address
    /// configured for yet (see PRD §9.4) — we don't guess one.
    pub sep7_uri: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreatePaymentRequestRequest>,
) -> ApiResult<Json<PaymentRequestView>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request("no merchant associated with this account"))?;

    let wallet = wallets::wallet_by_merchant(&state.db, merchant_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| bad_request("create a wallet before generating payment requests"))?;

    // Defaults to XLM, not cNGN like withdrawals: XLM is what's actually
    // scannable/testable today (no cNGN issuer address configured yet).
    let asset = req.asset.unwrap_or_else(|| "XLM".into());

    let pr = payment_requests::create_payment_request(
        &state.db,
        merchant_id,
        wallet.id,
        req.amount_stroops,
        asset,
        req.expires_in_secs,
    )
    .await
    .map_err(map_payment_request_error)?;

    Ok(Json(to_view(&pr, &wallet.address, &wallet.network)))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PaymentRequestView>> {
    let pr = payment_requests::payment_request_by_id(&state.db, id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found("payment request not found"))?;

    let wallet = wallets::wallet_by_id(&state.db, pr.wallet_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("payment request references a missing wallet"))?;

    Ok(Json(to_view(&pr, &wallet.address, &wallet.network)))
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<PaymentRequestView>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request("no merchant associated with this account"))?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);

    let rows = payment_requests::payment_requests_by_merchant(&state.db, merchant_id, limit)
        .await
        .map_err(internal)?;

    Ok(Json(rows.iter().map(row_to_view).collect()))
}

#[derive(serde::Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
}

/// Forces a `pending` request to `expired` immediately, merchant-scoped —
/// e.g. a customer abandoned the payment session and the merchant wants to
/// regenerate the QR without waiting out the TTL.
pub async fn expire(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PaymentRequestView>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request("no merchant associated with this account"))?;

    let pr = payment_requests::expire(&state.db, id, merchant_id)
        .await
        .map_err(map_expire_error)?;

    let wallet = wallets::wallet_by_id(&state.db, pr.wallet_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("payment request references a missing wallet"))?;

    Ok(Json(to_view(&pr, &wallet.address, &wallet.network)))
}

/// A `pending` row whose expiry has passed is reported as `expired` at read
/// time, so a request going stale needs no background job to flip it.
fn effective_status(status: &str, expires_at: DateTime<Utc>) -> String {
    if status == "pending" && expires_at < Utc::now() {
        "expired".to_string()
    } else {
        status.to_string()
    }
}

fn to_view(pr: &PaymentRequest, address: &str, network: &str) -> PaymentRequestView {
    PaymentRequestView {
        id: pr.id,
        merchant_id: pr.merchant_id,
        address: address.to_string(),
        network: network.to_string(),
        amount_stroops: pr.amount_stroops,
        asset: pr.asset.clone(),
        memo: pr.memo.clone(),
        status: effective_status(&pr.status, pr.expires_at),
        expires_at: pr.expires_at,
        created_at: pr.created_at,
        sep7_uri: build_sep7_uri(address, pr.amount_stroops, &pr.asset, &pr.memo),
    }
}

fn row_to_view(row: &payment_requests::PaymentRequestWithWallet) -> PaymentRequestView {
    PaymentRequestView {
        id: row.id,
        merchant_id: row.merchant_id,
        address: row.address.clone(),
        network: row.network.clone(),
        amount_stroops: row.amount_stroops,
        asset: row.asset.clone(),
        memo: row.memo.clone(),
        status: effective_status(&row.status, row.expires_at),
        expires_at: row.expires_at,
        created_at: row.created_at,
        sep7_uri: build_sep7_uri(&row.address, row.amount_stroops, &row.asset, &row.memo),
    }
}

fn build_sep7_uri(address: &str, amount_stroops: i64, asset: &str, memo: &str) -> Option<String> {
    if asset != "XLM" && asset != "native" {
        return None;
    }
    let amount = format!("{}.{:07}", amount_stroops / 10_000_000, amount_stroops % 10_000_000);
    Some(format!(
        "web+stellar:pay?destination={address}&amount={amount}&memo={memo}&memo_type=MEMO_TEXT"
    ))
}

fn map_payment_request_error(
    err: payment_requests::PaymentRequestError,
) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        payment_requests::PaymentRequestError::InvalidAmount => {
            bad_request("amount_stroops must be positive")
        }
        payment_requests::PaymentRequestError::Database(e) => internal(e),
    }
}

fn map_expire_error(
    err: payment_requests::ExpireError,
) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        // Not found and "belongs to another merchant" are indistinguishable
        // to the caller, same as every other merchant-scoped lookup here.
        payment_requests::ExpireError::NotFound => not_found("payment request not found"),
        payment_requests::ExpireError::NotPending => {
            bad_request("only a pending payment request can be expired")
        }
        payment_requests::ExpireError::Database(e) => internal(e),
    }
}
