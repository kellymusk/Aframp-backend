//! Partner-facing API handlers (Issue #408).
//!
//! All routes require `Authorization: Bearer <partner-api-key>`.
//!
//! POST /api/partner/quote
//! POST /api/partner/transfers
//! GET  /api/partner/transfers/:id
//! POST /api/partner/webhooks/test
//! GET  /api/partner/liquidity
//! GET  /api/partner/settlements
//! GET  /api/partner/branding

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use bigdecimal::{BigDecimal, FromPrimitive};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::database::partner_repository::PartnerRepository;
use crate::services::partner::{FxQuote, PartnerError, PartnerService};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct PartnerApiState {
    pub service: Arc<PartnerService>,
    pub repo: Arc<PartnerRepository>,
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct QuoteRequest {
    pub from_currency: String,
    pub to_currency: String,
    pub from_amount: String,
}

#[derive(Debug, Serialize)]
pub struct QuoteResponse {
    pub from_currency: String,
    pub to_currency: String,
    pub from_amount: String,
    pub to_amount: String,
    pub fx_rate: String,
    pub fee_amount: String,
    pub fee_type: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
pub struct TransferRequest {
    pub partner_ref: String,
    pub from_currency: String,
    pub to_currency: String,
    pub from_amount: String,
    pub fx_rate: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct TransferResponse {
    pub id: Uuid,
    pub partner_ref: String,
    pub from_currency: String,
    pub to_currency: String,
    pub from_amount: String,
    pub to_amount: String,
    pub fee_amount: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ---------------------------------------------------------------------------
// Auth helper
// ---------------------------------------------------------------------------

fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

fn err_resp(status: StatusCode, code: &str, msg: &str) -> Response {
    (
        status,
        Json(json!({"error": {"code": code, "message": msg}})),
    )
        .into_response()
}

fn partner_err(e: PartnerError) -> Response {
    match e {
        PartnerError::Unauthorized => err_resp(
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
            "Invalid or inactive API key",
        ),
        PartnerError::UnsupportedCorridor(c) => err_resp(
            StatusCode::BAD_REQUEST,
            "UNSUPPORTED_CORRIDOR",
            &format!("Corridor not supported: {}", c),
        ),
        PartnerError::BelowMinimum(m) => err_resp(
            StatusCode::BAD_REQUEST,
            "BELOW_MINIMUM",
            &format!("Amount below minimum: {}", m),
        ),
        PartnerError::ExceedsMaximum(m) => err_resp(
            StatusCode::BAD_REQUEST,
            "EXCEEDS_MAXIMUM",
            &format!("Amount exceeds maximum: {}", m),
        ),
        PartnerError::DailyLimitExceeded => err_resp(
            StatusCode::UNPROCESSABLE_ENTITY,
            "DAILY_LIMIT_EXCEEDED",
            "Daily volume limit exceeded",
        ),
        PartnerError::InsufficientLiquidity(c) => err_resp(
            StatusCode::UNPROCESSABLE_ENTITY,
            "INSUFFICIENT_LIQUIDITY",
            &format!("Insufficient liquidity for {}", c),
        ),
        PartnerError::DuplicateRef(r) => err_resp(
            StatusCode::CONFLICT,
            "DUPLICATE_REF",
            &format!("Duplicate partner_ref: {}", r),
        ),
        PartnerError::Database(e) => {
            err_resp(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", &e)
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn get_quote(
    State(state): State<Arc<PartnerApiState>>,
    headers: HeaderMap,
    Json(body): Json<QuoteRequest>,
) -> Response {
    let key = match extract_bearer(&headers) {
        Some(k) => k,
        None => {
            return err_resp(
                StatusCode::UNAUTHORIZED,
                "MISSING_KEY",
                "Authorization header required",
            )
        }
    };
    let partner = match state.service.authenticate(key).await {
        Ok(p) => p,
        Err(e) => return partner_err(e),
    };

    let from_amount = match body.from_amount.parse::<BigDecimal>() {
        Ok(a) => a,
        Err(_) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "INVALID_AMOUNT",
                "Invalid from_amount",
            )
        }
    };
    let base_rate = match body.from_currency.as_str() {
        // Placeholder rates — in production these come from ExchangeRateService
        "NGN" if body.to_currency == "KES" => BigDecimal::from_f64(0.28).unwrap(),
        "NGN" if body.to_currency == "GHS" => BigDecimal::from_f64(0.045).unwrap(),
        "KES" if body.to_currency == "NGN" => BigDecimal::from_f64(3.57).unwrap(),
        _ => BigDecimal::from_f64(1.0).unwrap(),
    };

    match state
        .service
        .get_quote(
            partner.id,
            &body.from_currency,
            &body.to_currency,
            from_amount,
            base_rate,
        )
        .await
    {
        Ok(q) => (
            StatusCode::OK,
            Json(QuoteResponse {
                from_currency: q.from_currency,
                to_currency: q.to_currency,
                from_amount: q.from_amount.to_string(),
                to_amount: q.to_amount.to_string(),
                fx_rate: q.fx_rate.to_string(),
                fee_amount: q.fee_amount.to_string(),
                fee_type: q.fee_type,
                expires_at: q.expires_at,
            }),
        )
            .into_response(),
        Err(e) => partner_err(e),
    }
}

pub async fn initiate_transfer(
    State(state): State<Arc<PartnerApiState>>,
    headers: HeaderMap,
    Json(body): Json<TransferRequest>,
) -> Response {
    let key = match extract_bearer(&headers) {
        Some(k) => k,
        None => {
            return err_resp(
                StatusCode::UNAUTHORIZED,
                "MISSING_KEY",
                "Authorization header required",
            )
        }
    };
    let partner = match state.service.authenticate(key).await {
        Ok(p) => p,
        Err(e) => return partner_err(e),
    };

    let from_amount = match body.from_amount.parse::<BigDecimal>() {
        Ok(a) => a,
        Err(_) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "INVALID_AMOUNT",
                "Invalid from_amount",
            )
        }
    };
    let fx_rate = match body.fx_rate.parse::<BigDecimal>() {
        Ok(r) => r,
        Err(_) => return err_resp(StatusCode::BAD_REQUEST, "INVALID_RATE", "Invalid fx_rate"),
    };

    // Re-compute quote from provided rate (partner locked in the rate from /quote)
    let fee_amount = BigDecimal::from(0u64); // fees already in quote; use 0 for re-initiation
    let to_amount = &from_amount * &fx_rate;
    let quote = FxQuote {
        from_currency: body.from_currency.clone(),
        to_currency: body.to_currency.clone(),
        from_amount,
        to_amount,
        fx_rate,
        fee_amount,
        fee_type: "flat".to_string(),
        expires_at: chrono::Utc::now(),
    };

    match state
        .service
        .initiate_transfer(
            partner.id,
            &body.partner_ref,
            &quote,
            body.metadata
                .unwrap_or(serde_json::Value::Object(Default::default())),
        )
        .await
    {
        Ok(t) => {
            (
                StatusCode::CREATED,
                Json(TransferResponse {
                    id: t.id,
                    partner_ref: t.partner_ref,
                    from_currency: t.from_currency,
                    to_currency: t.to_currency,
                    from_amount: t.from_amount.to_string(),
                    to_amount: t.to_amount.to_string(),
                    fee_amount: t.fee_amount.to_string(),
                    status: t.status,
                    created_at: t.created_at,
                }),
            )
                .into_response()
        }
        Err(e) => partner_err(e),
    }
}

pub async fn get_transfer_status(
    State(state): State<Arc<PartnerApiState>>,
    headers: HeaderMap,
    Path(transfer_id): Path<Uuid>,
) -> Response {
    let key = match extract_bearer(&headers) {
        Some(k) => k,
        None => {
            return err_resp(
                StatusCode::UNAUTHORIZED,
                "MISSING_KEY",
                "Authorization header required",
            )
        }
    };
    let partner = match state.service.authenticate(key).await {
        Ok(p) => p,
        Err(e) => return partner_err(e),
    };

    match state.repo.get_transfer(transfer_id, partner.id).await {
        Ok(Some(t)) => (
            StatusCode::OK,
            Json(TransferResponse {
                id: t.id,
                partner_ref: t.partner_ref,
                from_currency: t.from_currency,
                to_currency: t.to_currency,
                from_amount: t.from_amount.to_string(),
                to_amount: t.to_amount.to_string(),
                fee_amount: t.fee_amount.to_string(),
                status: t.status,
                created_at: t.created_at,
            }),
        )
            .into_response(),
        Ok(None) => err_resp(StatusCode::NOT_FOUND, "NOT_FOUND", "Transfer not found"),
        Err(e) => err_resp(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            &e.to_string(),
        ),
    }
}

pub async fn get_liquidity(
    State(state): State<Arc<PartnerApiState>>,
    headers: HeaderMap,
) -> Response {
    let key = match extract_bearer(&headers) {
        Some(k) => k,
        None => {
            return err_resp(
                StatusCode::UNAUTHORIZED,
                "MISSING_KEY",
                "Authorization header required",
            )
        }
    };
    let partner = match state.service.authenticate(key).await {
        Ok(p) => p,
        Err(e) => return partner_err(e),
    };

    match state.repo.list_liquidity(partner.id).await {
        Ok(accounts) => {
            let items: Vec<serde_json::Value> = accounts
                .iter()
                .map(|a| {
                    json!({
                        "currency": a.currency,
                        "balance": a.balance.to_string(),
                        "reserved": a.reserved.to_string(),
                        "available": (&a.balance - &a.reserved).to_string(),
                        "stellar_address": a.stellar_address,
                    })
                })
                .collect();
            (StatusCode::OK, Json(json!({"liquidity": items}))).into_response()
        }
        Err(e) => err_resp(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            &e.to_string(),
        ),
    }
}

pub async fn get_settlements(
    State(state): State<Arc<PartnerApiState>>,
    headers: HeaderMap,
) -> Response {
    let key = match extract_bearer(&headers) {
        Some(k) => k,
        None => {
            return err_resp(
                StatusCode::UNAUTHORIZED,
                "MISSING_KEY",
                "Authorization header required",
            )
        }
    };
    let partner = match state.service.authenticate(key).await {
        Ok(p) => p,
        Err(e) => return partner_err(e),
    };

    match state.repo.list_settlements(partner.id, 30).await {
        Ok(settlements) => {
            let items: Vec<serde_json::Value> = settlements
                .iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "settlement_date": s.settlement_date.to_string(),
                        "total_volume": s.total_volume.to_string(),
                        "total_fees": s.total_fees.to_string(),
                        "net_payable": s.net_payable.to_string(),
                        "tx_count": s.tx_count,
                        "status": s.status,
                        "report_url": s.report_url,
                    })
                })
                .collect();
            (StatusCode::OK, Json(json!({"settlements": items}))).into_response()
        }
        Err(e) => err_resp(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            &e.to_string(),
        ),
    }
}

pub async fn get_branding(
    State(state): State<Arc<PartnerApiState>>,
    headers: HeaderMap,
) -> Response {
    let key = match extract_bearer(&headers) {
        Some(k) => k,
        None => {
            return err_resp(
                StatusCode::UNAUTHORIZED,
                "MISSING_KEY",
                "Authorization header required",
            )
        }
    };
    let partner = match state.service.authenticate(key).await {
        Ok(p) => p,
        Err(e) => return partner_err(e),
    };

    match state.repo.get_branding(partner.id).await {
        Ok(Some(b)) => (
            StatusCode::OK,
            Json(json!({
                "logo_url": b.logo_url,
                "primary_color": b.primary_color,
                "secondary_color": b.secondary_color,
                "email_template": b.email_template,
                "language_overrides": b.language_overrides,
            })),
        )
            .into_response(),
        Ok(None) => (StatusCode::OK, Json(json!({}))).into_response(),
        Err(e) => err_resp(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            &e.to_string(),
        ),
    }
}
