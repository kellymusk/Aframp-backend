use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    error::AppError,
    state::AppState,
};

#[derive(Debug, Serialize, FromRow)]
pub struct SessionResponse {
    pub id: Uuid,
    pub device: Option<String>,
    pub ip: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub current: bool,
}

/// GET /sessions
///
/// List the authenticated merchant's active sessions.
pub async fn list_sessions(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let rows = sqlx::query_as::<_, SessionResponse>(
        r#"
        SELECT id, device, ip, created_at, last_seen
        FROM sessions
        WHERE merchant_id = $1 AND revoked_at IS NULL
        ORDER BY last_seen DESC
        "#,
    )
    .bind(auth.merchant_id)
    .fetch_all(&state.db)
    .await?;

    let sessions = rows
        .into_iter()
        .map(|mut s| {
            s.current = s.id == auth.session_id;
            s
        })
        .collect::<Vec<_>>();

    Ok(Json(sessions))
}

/// DELETE /sessions/{id}
///
/// Revoke a specific session. Revoked sessions stop working immediately
/// because the auth middleware checks the revocation list.
pub async fn revoke_session(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE sessions
        SET revoked_at = NOW()
        WHERE id = $1 AND merchant_id = $2 AND revoked_at IS NULL
        "#,
    )
    .bind(id)
    .bind(auth.merchant_id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}
