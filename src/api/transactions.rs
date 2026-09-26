use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::stream;
use serde::Deserialize;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, ApiResult, ErrorCode};
use crate::models::Payment;
use crate::pagination::{Cursor, Page};
use crate::services::payments;
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<Payment>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(Cursor::decode(raw).ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?),
        None => None,
    };
    let payments = payments::payments_by_merchant_cursor(&state.db, merchant_id, limit, cursor)
        .await
        .map_err(internal)?;
    Ok(Json(Page::new(payments, limit, |p| Cursor {
        created_at: p.created_at,
        id: p.id,
    })))
}

#[derive(Deserialize)]
pub struct ExportParams {
    pub format: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn payment_csv_row(p: &Payment) -> String {
    format!(
        "{},{},{},{},{},{}\n",
        p.id,
        csv_escape(&p.status),
        p.amount,
        csv_escape(&p.currency),
        p.created_at.to_rfc3339(),
        csv_escape(p.description.as_deref().unwrap_or("")),
    )
}

pub async fn export(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ExportParams>,
) -> ApiResult<Response> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;

    if let Some(format) = params.format.as_deref() {
        if !format.eq_ignore_ascii_case("csv") {
            return Err(bad_request(ErrorCode::InvalidParameters, "unsupported export format"));
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

    let header = "id,status,amount,currency,created_at,description\n".to_string();
    let body_stream = stream::iter(
        std::iter::once(header)
            .chain(rows.iter().map(payment_csv_row)),
    );

    let filename = match (from, to) {
        (Some(f), Some(t)) => format!("transactions_{}_{}.csv", f.format("%Y%m%d"), t.format("%Y%m%d")),
        _ => "transactions.csv".to_string(),
    };

    let response = Response::builder()
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(Body::from_stream(body_stream))
        .map_err(|_| internal(anyhow::anyhow!("failed to build export response")))?;

    Ok(response)
}
