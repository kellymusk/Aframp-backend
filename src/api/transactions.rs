use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap};
use axum::response::Response;
use futures_util::stream;
use serde::Deserialize;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, ApiResult, ErrorCode};
use crate::etag;
use crate::models::{ListParams, Payment};
use crate::pagination::{Cursor, Page};
use crate::services::payments;
use crate::AppState;

/// Payment statuses accepted by the `status` filter on `GET /transactions`.
pub const TRANSACTION_STATUSES: [&str; 4] = ["detected", "verified", "confirmed", "failed"];

#[derive(Deserialize)]
pub struct TransactionListParams {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    pub asset: Option<String>,
    pub status: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<TransactionListParams>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let asset = match query.asset.as_deref().map(str::trim) {
        Some("") => {
            return Err(bad_request(
                ErrorCode::InvalidParameters,
                "asset must not be empty",
            ))
        }
        other => other,
    };
    let status = match query.status.as_deref().map(str::trim) {
        Some(s) if !TRANSACTION_STATUSES.contains(&s) => {
            return Err(bad_request(
                ErrorCode::InvalidParameters,
                "status must be one of detected, verified, confirmed, failed",
            ))
        }
        other => other,
    };
    let params = ListParams {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
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
    let payments =
        payments::payments_by_merchant_cursor(&state.db, merchant_id, limit, cursor, asset, status)
            .await
            .map_err(internal)?;
    let page = Page::new(payments, limit, |p| Cursor {
        created_at: p.created_at,
        id: p.id,
    });
    Ok(etag::conditional_json(&headers, &page))
}

#[derive(Deserialize)]
pub struct ExportParams {
    pub format: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

pub async fn export(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ExportParams>,
) -> ApiResult<Response> {
    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })?;

    if let Some(format) = params.format.as_deref() {
        if !format.eq_ignore_ascii_case("csv") {
            return Err(bad_request(
                ErrorCode::InvalidParameters,
                "unsupported export format",
            ));
        }
    }

    let from = match params.from.as_deref() {
        Some(raw) => Some(
            chrono::DateTime::parse_from_rfc3339(raw)
                .map_err(|_| bad_request(ErrorCode::InvalidParameters, "invalid 'from' timestamp"))?
                .with_timezone(&chrono::Utc),
        ),
        None => None,
    };
    let to = match params.to.as_deref() {
        Some(raw) => Some(
            chrono::DateTime::parse_from_rfc3339(raw)
                .map_err(|_| bad_request(ErrorCode::InvalidParameters, "invalid 'to' timestamp"))?
                .with_timezone(&chrono::Utc),
        ),
        None => None,
    };

    let rows = payments::payments_by_merchant_range(&state.db, merchant_id, from, to)
        .await
        .map_err(internal)?;

    let header = format!("{}\n", Payment::CSV_HEADER);
    let body_stream = stream::iter(
        std::iter::once(header)
            .chain(rows.into_iter().map(|p| format!("{}\n", p.to_csv_record())))
            .map(Ok::<_, std::convert::Infallible>),
    );

    let filename = match (from, to) {
        (Some(f), Some(t)) => format!(
            "transactions_{}_{}.csv",
            f.format("%Y%m%d"),
            t.format("%Y%m%d")
        ),
        _ => "transactions.csv".to_string(),
    };

    let response = Response::builder()
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(Body::from_stream(body_stream))
        .map_err(|_| internal("failed to build export response"))?;

    Ok(response)
}
