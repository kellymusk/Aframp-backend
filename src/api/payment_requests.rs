use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, not_found, ApiResult, ErrorCode};
use crate::models::{CreatePaymentRequestRequest, PaymentRequest};
use crate::pagination::{Cursor, Page};
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
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;

    let wallet = wallets::wallet_by_merchant(&state.db, merchant_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| bad_request(ErrorCode::WalletNotFound, "create a wallet before generating payment requests"))?;

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
        .ok_or_else(|| not_found(ErrorCode::PaymentRequestNotFound, "payment request not found"))?;

    let wallet = wallets::wallet_by_id(&state.db, pr.wallet_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("payment request references a missing wallet"))?;

    Ok(Json(to_view(&pr, &wallet.address, &wallet.network)))
}

/// Renders the payment request's SEP-0007 URI as a PNG QR code so merchants
/// can display it directly at a POS terminal without a client-side QR library.
///
/// The rendered image is cached in-process keyed by `(id, size)`; the SEP-0007
/// URI for a given request is immutable, so the cache never needs invalidation.
pub async fn qr(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<QrParams>,
) -> ApiResult<impl IntoResponse> {
    let size = params.size.unwrap_or(256).clamp(64, 1024) as u32;

    let pr = payment_requests::payment_request_by_id(&state.db, id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::PaymentRequestNotFound, "payment request not found"))?;

    let wallet = wallets::wallet_by_id(&state.db, pr.wallet_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("payment request references a missing wallet"))?;

    let sep7_uri = build_sep7_uri(&wallet.address, pr.amount_stroops, &pr.asset, &pr.memo)
        .ok_or_else(|| {
            bad_request(
                ErrorCode::InvalidParameters,
                "this payment request has no SEP-0007 URI to encode",
            )
        })?;

    let png = qr_png_cached(id, size, &sep7_uri)?;

    Ok(([(header::CONTENT_TYPE, "image/png")], png))
}

#[derive(serde::Deserialize)]
pub struct QrParams {
    pub size: Option<i64>,
}

/// In-process cache of rendered QR PNGs keyed by `(payment request id, size)`.
fn qr_cache() -> &'static Mutex<HashMap<(Uuid, u32), Vec<u8>>> {
    static CACHE: OnceLock<Mutex<HashMap<(Uuid, u32), Vec<u8>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn qr_png_cached(id: Uuid, size: u32, contents: &str) -> ApiResult<Vec<u8>> {
    let key = (id, size);
    if let Some(cached) = qr_cache().lock().unwrap().get(&key) {
        return Ok(cached.clone());
    }

    let png = render_qr_png(contents, size)?;
    qr_cache().lock().unwrap().insert(key, png.clone());
    Ok(png)
}

fn render_qr_png(contents: &str, size: u32) -> ApiResult<Vec<u8>> {
    use qrcode::QrCode;

    let code = QrCode::new(contents.as_bytes())
        .map_err(|_| internal("failed to encode SEP-0007 URI as a QR code"))?;
    let image = code
        .render::<image::Luma<u8>>()
        .min_dimensions(size, size)
        .build();

    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|_| internal("failed to encode QR code as PNG"))?;
    Ok(png)
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<PaymentRequestView>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(Cursor::decode(raw).ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?),
        None => None,
    };

    let rows =
        payment_requests::payment_requests_by_merchant_cursor(&state.db, merchant_id, limit, cursor)
            .await
            .map_err(internal)?;

    Ok(Json(Page::new(
        rows.iter().map(row_to_view).collect(),
        limit,
        |v: &PaymentRequestView| Cursor {
            created_at: v.created_at,
            id: v.id,
        },
    )))
}

#[derive(serde::Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
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
            bad_request(ErrorCode::InvalidAmount, "amount_stroops must be positive")
        }
        payment_requests::PaymentRequestError::Database(e) => internal(e),
    }
}
