use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::Json;

use crate::auth::{cookie, jwt};
use crate::auth::jwt::Claims;
use crate::error::{forbidden, internal, ApiError, ErrorCode};
use crate::services::users;
use crate::AppState;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: uuid::Uuid,
    pub merchant_id: Option<uuid::Uuid>,
    pub via: AuthMethod,
}

/// How the caller proved who they are. Most handlers don't care, but minting
/// an API key is session-only, so a leaked key cannot mint its own
/// replacement and outlive its revocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// A session JWT, from the `Authorization` header or the session cookie.
    Session,
    /// A long-lived `sk_`-prefixed API key.
    ApiKey,
}

/// The verified claims of the presented session token (bearer or cookie),
/// for handlers that need more than the user id — e.g. token refresh.
#[derive(Debug)]
pub struct Session(pub Claims);

impl FromRequestParts<AppState> for Session {
    type Rejection = (StatusCode, Json<ApiError>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        authenticate_active(parts, state).await.map(Session)
    }
}

/// Same session proof as [`AuthUser`], but additionally requires admin rights.
/// The `is_admin` JWT claim is only a cheap first filter: every admin request
/// also re-reads `users.is_admin`, so revoking admin access in the database
/// takes effect on the very next request rather than when the token expires.
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

/// Resolve a merchant-scoped API key (`sk_test_…` / `sk_live_…`) into an
/// [`AuthUser`]. Returns `Ok(None)` for bearer values that aren't shaped like
/// one of our keys, so a JWT presented on the same header falls through.
async fn authenticate_api_key(
    state: &AppState,
    token: &str,
) -> Result<Option<AuthUser>, (StatusCode, Json<ApiError>)> {
    if !token.starts_with("sk_") {
        return Ok(None);
    }
    let principal = crate::services::api_keys::authenticate(&state.db, token)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ApiError {
                    code: ErrorCode::InvalidCredentials,
                    error: "invalid or revoked api key".into(),
                    field: None,
                    retry_after_secs: None,
                }),
            )
        })?;
    Ok(Some(AuthUser {
        user_id: principal.user_id,
        merchant_id: Some(principal.merchant_id),
        via: AuthMethod::ApiKey,
    }))
}

fn authenticate(parts: &Parts, state: &AppState) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    // API clients send a bearer token; browsers send the HttpOnly session
    // cookie, which JS on the page cannot read. Either proves the session.
    let token = bearer_token(parts)
        .or_else(|| cookie::from_headers(&parts.headers))
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, Json(ApiError { code: ErrorCode::InvalidCredentials, error: "missing session cookie or bearer token".into(), field: None, retry_after_secs: None })))?;
    jwt::verify(&state.jwt_secret, token)
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ApiError { code: ErrorCode::InvalidCredentials, error: "invalid or expired token".into(), field: None, retry_after_secs: None })))
}

/// Verifies the token and that its account still exists and hasn't been
/// deleted, so deleting an account revokes every token issued for it.
async fn authenticate_active(
    parts: &Parts,
    state: &AppState,
) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    let claims = authenticate(parts, state)?;
    if !users::is_active(&state.db, claims.sub).await.map_err(internal)? {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiError { code: ErrorCode::InvalidCredentials, error: "invalid or expired token".into(), field: None, retry_after_secs: None }),
        ));
    }
    Ok(claims)
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
        let claims = authenticate_active(parts, state).await?;
        Ok(AuthUser {
            user_id: claims.sub,
            merchant_id: claims.merchant_id,
            via: AuthMethod::Session,
        })
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = (StatusCode, Json<ApiError>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let claims = authenticate_active(parts, state).await?;
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
