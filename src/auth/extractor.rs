use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::Json;

use crate::auth::{cookie, jwt};
use crate::auth::jwt::Claims;
use crate::error::{forbidden, ApiError, ErrorCode};
use crate::AppState;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: uuid::Uuid,
    pub merchant_id: Option<uuid::Uuid>,
}

/// Same session proof as [`AuthUser`], but additionally requires the `is_admin`
/// JWT claim. The claim is baked in at login and not re-checked against the
/// database, so revoking admin access takes up to [`jwt::TOKEN_TTL_HOURS`] to
/// take effect on outstanding tokens.
#[derive(Debug, Clone)]
pub struct AdminUser;

/// Extract the raw bearer token from the `Authorization` header, if present.
fn bearer_token(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Resolve a merchant-scoped API key (`sk_test_` / `sk_live_` prefix) into an
/// [`AuthUser`]. Only the hash of the key is stored, so we hash the presented
/// token and look it up. Revoked keys are rejected.
async fn authenticate_api_key(
    state: &AppState,
    token: &str,
) -> Result<Option<AuthUser>, (StatusCode, Json<ApiError>)> {
    if !token.starts_with("sk_test_") && !token.starts_with("sk_live_") {
        return Ok(None);
    }
    let key_hash = crate::auth::api_key::hash(token);
    let row = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid)>(
        "SELECT id, merchant_id FROM api_keys WHERE key_hash = $1 AND revoked_at IS NULL",
    )
    .bind(&key_hash)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                code: ErrorCode::Internal,
                error: "failed to verify api key".into(),
                field: None,
            }),
        )
    })?;

    let Some((_key_id, merchant_id)) = row else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiError {
                code: ErrorCode::InvalidCredentials,
                error: "invalid or revoked api key".into(),
                field: None,
            }),
        ));
    };

    Ok(Some(AuthUser {
        user_id: merchant_id,
        merchant_id: Some(merchant_id),
    }))
}

fn authenticate(parts: &Parts, state: &AppState) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    // API clients send a bearer token; browsers send the HttpOnly session
    // cookie, which JS on the page cannot read. Either proves the session.
    let token = bearer_token(parts)
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
        // Merchant API keys take precedence over session JWTs so server-to-server
        // integrations can authenticate without an OTP login.
        if let Some(token) = bearer_token(parts) {
            if let Some(user) = authenticate_api_key(state, token).await? {
                return Ok(user);
            }
        }
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
        tracing::info!(admin_user_id = %claims.sub, path = %parts.uri.path(), "admin access");
        Ok(AdminUser)
    }
}
