use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ApiError;

#[derive(Debug, Deserialize)]
pub struct ListTransactionsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub status: Option<String>,
    pub q: Option<String>,
    pub memo: Option<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TransactionRow {
    pub id: Uuid,
    pub payment_request_id: Option<Uuid>,
    pub memo: Option<String>,
    pub amount: i64,
    pub currency: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn list_transactions(
    State(pool): State<PgPool>,
    Query(params): Query<ListTransactionsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);

    // `q` and `memo` both search the linked payment request's memo/reference.
    let memo_filter = params.memo.or(params.q);

    let rows = sqlx::query_as::<_, TransactionRow>(
        r#"
        SELECT
            t.id,
            t.payment_request_id,
            pr.memo AS memo,
            t.amount,
            t.currency,
            t.status,
            t.created_at
        FROM transactions t
        LEFT JOIN payment_requests pr ON pr.id = t.payment_request_id
        WHERE ($1::text IS NULL OR t.status = $1)
          AND ($2::text IS NULL OR pr.memo ILIKE '%' || $2 || '%')
        ORDER BY t.created_at DESC
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(params.status.as_deref())
    .bind(memo_filter.as_deref())
    .bind(limit)
    .bind(offset)
    .fetch_all(&pool)
    .await?;

    Ok((StatusCode::OK, Json(rows)))
}
