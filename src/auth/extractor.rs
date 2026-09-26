use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::Json;

use crate::auth::{cookie, jwt};
use crate::auth::jwt::Claims;
use crate::error::{forbidden, internal, ApiError, ErrorCode};
use crate::AppState;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: uuid::Uuid,
    pub merchant_id: Option<uuid::Uuid>,
}

/// Same session proof as [`AuthUser`], but additionally requires admin rights.
/// The `is_admin` JWT claim is only a cheap first filter: every admin request
/// also re-reads `users.is_admin`, so revoking admin access in the database
/// takes effect on the very next request rather than when the token expires.
#[derive(Debug, Clone)]
pub struct AdminUser;

fn authenticate(parts: &Parts, state: &AppState) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    // API clients send a bearer token; browsers send the HttpOnly session
    // cookie, which JS on the page cannot read. Either proves the session.
    let token = parts
        .headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| cookie::from_headers(&parts.headers))
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, Json(ApiError { code: ErrorCode::InvalidCredentials, error: "missing session cookie or bearer token".into(), field: None })))?;
    jwt::verify(&state.jwt_secret, token)
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ApiError { code: ErrorCode::InvalidCredentials, error: "invalid or expired token".into(), field: None })))
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = (StatusCode, Json<ApiError>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let claims = authenticate(parts, state)?;
        Ok(AuthUser {
            user_id: claims.sub,
            merchant_id: claims.merchant_id,
        })
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = (StatusCode, Json<ApiError>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let claims = authenticate(parts, state)?;
        if !claims.is_admin {
            return Err(forbidden(ErrorCode::Forbidden, "admin access required"));
        }
        let still_admin: Option<bool> = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(internal)?;
        if still_admin != Some(true) {
            tracing::warn!(user_id = %claims.sub, "admin token presented after admin access was revoked");
            return Err(forbidden(ErrorCode::Forbidden, "admin access required"));
        }
        tracing::info!(admin_user_id = %claims.sub, path = %parts.uri.path(), "admin access");
        Ok(AdminUser)
    }
}
