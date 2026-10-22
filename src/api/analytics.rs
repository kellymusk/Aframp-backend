use axum::extract::{Query, State};
use axum::Json;
use chrono::{Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, ApiResult, ErrorCode};
use crate::services::payments::{self, DailyPaymentAggregate};
use crate::AppState;

/// Widest window one request may cover, in days (inclusive of both ends).
const MAX_WINDOW_DAYS: i64 = 366;
/// Default window when `from` is omitted: today and the 29 days before it.
const DEFAULT_WINDOW_DAYS: i64 = 30;

#[derive(Deserialize)]
pub struct AnalyticsParams {
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Serialize)]
pub struct PaymentAnalytics {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub data: Vec<DailyPaymentAggregate>,
}

fn parse_date(name: &str, value: &str) -> ApiResult<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
        bad_request(
            ErrorCode::InvalidParameters,
            &format!("{name} must be a YYYY-MM-DD date"),
        )
    })
}

/// `GET /analytics/payments` — daily totals of the merchant's confirmed
/// payments per asset, UTC days, sparse (days with no payments are omitted).
pub async fn payments(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<AnalyticsParams>,
) -> ApiResult<Json<PaymentAnalytics>> {
    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })?;

    let to = match params.to.as_deref() {
        Some(raw) => parse_date("to", raw)?,
        None => Utc::now().date_naive(),
    };
    let from = match params.from.as_deref() {
        Some(raw) => parse_date("from", raw)?,
        None => to - Duration::days(DEFAULT_WINDOW_DAYS - 1),
    };
    if from > to {
        return Err(bad_request(
            ErrorCode::InvalidParameters,
            "from must not be after to",
        ));
    }
    if (to - from).num_days() + 1 > MAX_WINDOW_DAYS {
        return Err(bad_request(
            ErrorCode::InvalidParameters,
            "the date window must not exceed 366 days",
        ));
    }

    let start = from
        .and_hms_opt(0, 0, 0)
        .expect("midnight is valid")
        .and_utc();
    let end = (to + Duration::days(1))
        .and_hms_opt(0, 0, 0)
        .expect("midnight is valid")
        .and_utc();
    let data = payments::daily_confirmed_totals(&state.db, merchant_id, start, end)
        .await
        .map_err(internal)?;

    Ok(Json(PaymentAnalytics { from, to, data }))
}
