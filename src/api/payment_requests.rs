use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, not_found, ApiResult, ErrorCode};
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

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<PaymentRequestView>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
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
