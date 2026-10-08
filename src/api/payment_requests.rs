use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::IntoResponse;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use serde_json::Value;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{
    bad_request, bad_request_field, forbidden, internal, not_found, ApiResult, ErrorCode,
};
use crate::models::status::PaymentRequestStatus;
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
    pub cancelled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// SEP-0007 payment URI a Stellar wallet can open directly to pay this
    /// request. `None` for credit assets we don't have a real issuer address
    /// configured for yet (see PRD §9.4) — we don't guess one.
    pub sep7_uri: Option<String>,
}

/// Lightweight public polling payload for customers waiting on a QR payment.
#[derive(Serialize)]
pub struct PaymentRequestStatusView {
    pub status: String,
    /// Present when `status` is `paid` — the row's `updated_at` at mark-paid time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid_at: Option<DateTime<Utc>>,
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> ApiResult<Json<PaymentRequestView>> {
    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })?;
    let req = CreatePaymentRequestRequest::from_json(&body)
        .map_err(|(field, msg)| bad_request_field(field, msg))?;

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

    let wallet = wallets::wallet_by_merchant(&state.db, merchant_id, "stellar")
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            bad_request(
                ErrorCode::WalletNotFound,
                "create a wallet before generating payment requests",
            )
        })?;

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
    .await?;

    Ok(Json(to_view(&pr, &wallet.address, &wallet.network)))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PaymentRequestView>> {
    let pr = payment_requests::payment_request_by_id(&state.db, id)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            not_found(
                ErrorCode::PaymentRequestNotFound,
                "payment request not found",
            )
        })?;

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

/// Public, cache-friendly status-only poll for customer devices after they
/// scan a QR. Prefer this over `GET /payment-requests/{id}` when only the
/// payment outcome is needed.
pub async fn status(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<impl IntoResponse> {
    let pr = payment_requests::payment_request_by_id(&state.db, id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::PaymentRequestNotFound, "payment request not found"))?;

    let status = payment_requests::effective_status(pr.status, pr.expires_at, pr.cancelled_at);
    let paid_at = if status == "paid" {
        Some(pr.updated_at)
    } else {
        None
    };

    let mut response = Json(PaymentRequestStatusView { status, paid_at }).into_response();
    // Short TTL so CDN/browser can coalesce rapid polls without serving stale
    // "pending" for long after a payment flips to paid.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=5"),
    );
    Ok(response)
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
    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let include_cancelled = params.include_cancelled.unwrap_or(false);
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(
            Cursor::decode(raw)
                .ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?,
        ),
        None => None,
    };

    let rows = payment_requests::payment_requests_by_merchant_cursor(
        &state.db,
        merchant_id,
        limit,
        cursor,
        include_cancelled,
    )
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

/// `POST /payment-requests/{id}/expire` — end a pending request immediately.
pub async fn expire(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PaymentRequestView>> {
    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account")
    })?;
    let pr = payment_requests::expire(&state.db, id, merchant_id)
        .await
        .map_err(|err| match err {
            // Missing and "belongs to another merchant" look the same.
            payment_requests::ExpireError::NotFound => {
                not_found(ErrorCode::PaymentRequestNotFound, "payment request not found")
            }
            payment_requests::ExpireError::NotPending => bad_request(
                ErrorCode::InvalidParameters,
                "only a pending payment request can be expired",
            ),
            payment_requests::ExpireError::Database(e) => internal(e),
        })?;
    let wallet = wallets::wallet_by_id(&state.db, pr.wallet_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("payment request references a missing wallet"))?;
    Ok(Json(to_view(&pr, &wallet.address, &wallet.network)))
}

/// Soft-delete (archive) a payment request owned by the authenticated merchant.
/// Sets `cancelled_at`; the row remains until the cleanup job hard-deletes
/// expired+cancelled rows older than 30 days.
pub async fn cancel(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PaymentRequestView>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;

    // Distinguish not-found / not-owned / already-cancelled for clearer errors.
    let existing = payment_requests::payment_request_by_id(&state.db, id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::PaymentRequestNotFound, "payment request not found"))?;

    if existing.merchant_id != merchant_id {
        return Err(forbidden(
            ErrorCode::Forbidden,
            "cannot cancel another merchant's payment request",
        ));
    }
    if existing.cancelled_at.is_some() {
        return Err(bad_request(
            ErrorCode::InvalidParameters,
            "payment request is already cancelled",
        ));
    }

    let pr = payment_requests::cancel_payment_request(&state.db, id, merchant_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::PaymentRequestNotFound, "payment request not found"))?;

    let wallet = wallets::wallet_by_id(&state.db, pr.wallet_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("payment request references a missing wallet"))?;

    Ok(Json(to_view(&pr, &wallet.address, &wallet.network)))
}

#[derive(serde::Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    /// When true, include soft-deleted (cancelled) requests in the list.
    /// Default: false.
    pub include_cancelled: Option<bool>,
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
        status: payment_requests::effective_status(pr.status, pr.expires_at, pr.cancelled_at),
        expires_at: pr.expires_at,
        cancelled_at: pr.cancelled_at,
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
        status: payment_requests::effective_status(row.status, row.expires_at, row.cancelled_at),
        expires_at: row.expires_at,
        cancelled_at: row.cancelled_at,
        created_at: row.created_at,
        sep7_uri: build_sep7_uri(&row.address, row.amount_stroops, &row.asset, &row.memo),
    }
}

fn build_sep7_uri(address: &str, amount_stroops: i64, asset: &str, memo: &str) -> Option<String> {
    if asset != "XLM" && asset != "native" {
        return None;
    }
    let amount = format!(
        "{}.{:07}",
        amount_stroops / 10_000_000,
        amount_stroops % 10_000_000
    );
    Some(format!(
        "web+stellar:pay?destination={}&amount={}&memo={}&memo_type=MEMO_TEXT",
        percent_encode(address),
        percent_encode(&amount),
        percent_encode(memo),
    ))
}

/// Percent-encode a SEP-7 query value (RFC 3986): everything except
/// unreserved characters is escaped, so a value can never inject `&`, `=`
/// or `#` into the URI.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_params(uri: &str) -> Vec<(String, String)> {
        let query = uri.split_once('?').unwrap().1;
        query
            .split('&')
            .map(|pair| {
                let (k, v) = pair.split_once('=').unwrap();
                (k.to_string(), percent_decode(v))
            })
            .collect()
    }

    fn percent_decode(value: &str) -> String {
        let bytes = value.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                out.push(u8::from_str_radix(&value[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn sep7_uri_encodes_special_characters_in_memo() {
        let uri = build_sep7_uri("GABC", 10_000_000, "XLM", "a&b=c#d e").unwrap();
        assert_eq!(
            uri,
            "web+stellar:pay?destination=GABC&amount=1.0000000&memo=a%26b%3Dc%23d%20e&memo_type=MEMO_TEXT"
        );
        assert!(!uri.contains('#'));
    }

    #[test]
    fn sep7_uri_cannot_gain_injected_params() {
        let uri = build_sep7_uri("GABC", 1, "native", "x&destination=GEVIL").unwrap();
        let params = query_params(&uri);
        let keys: Vec<&str> = params.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["destination", "amount", "memo", "memo_type"]);
        assert_eq!(params[0].1, "GABC");
    }

    #[test]
    fn sep7_uri_round_trips_through_a_parser() {
        let memo = "pay #42 & tip = 5%";
        let uri = build_sep7_uri("GDEST", 25_000_000, "XLM", memo).unwrap();
        let params = query_params(&uri);
        assert_eq!(
            params,
            [
                ("destination".to_string(), "GDEST".to_string()),
                ("amount".to_string(), "2.5000000".to_string()),
                ("memo".to_string(), memo.to_string()),
                ("memo_type".to_string(), "MEMO_TEXT".to_string()),
            ]
        );
    }

    #[test]
    fn sep7_uri_is_none_for_non_native_assets() {
        assert!(build_sep7_uri("GABC", 1, "cNGN", "m").is_none());
    }
}
