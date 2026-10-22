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
    bearer_from_headers(&parts.headers)
}

fn bearer_from_headers(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            // The auth scheme is case-insensitive (RFC 9110 §11.1).
            let (scheme, token) = v.split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then(|| token.trim_start())
        })
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

/// The session token from the `Authorization: Bearer` header, falling back to
/// the session cookie. API clients send a bearer token; browsers send the
/// HttpOnly session cookie, which JS on the page cannot read.
pub(crate) fn session_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    bearer_from_headers(headers).or_else(|| cookie::from_headers(headers))
}

/// Verifies the session token and that it hasn't been revoked by a logout.
async fn authenticate(parts: &Parts, state: &AppState) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    let unauthorized = |msg: &str| {
        (
            StatusCode::UNAUTHORIZED,
            Json(ApiError {
                code: ErrorCode::InvalidCredentials,
                error: msg.into(),
                field: None,
                retry_after_secs: None,
            }),
        )
    };
    let token = session_token(&parts.headers)
        .ok_or_else(|| unauthorized("missing session cookie or bearer token"))?;
    jwt::verify_active(&state.db, &state.jwt_secret, token)
        .await
        .map_err(|err| match err {
            jwt::VerifyError::Database(e) => internal(e),
            _ => unauthorized("invalid or expired token"),
        })
}

/// Verifies the token and that its account still exists and hasn't been
/// deleted, so deleting an account revokes every token issued for it.
async fn authenticate_active(
    parts: &Parts,
    state: &AppState,
) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    let claims = authenticate(parts, state).await?;
    // Refreshed tokens keep their original `orig_iat`, so a refresh can't
    // carry a session past a password change.
    let session_start = if claims.orig_iat == 0 { claims.iat } else { claims.orig_iat } as i64;
    if !users::session_is_valid(&state.db, claims.sub, session_start)
        .await
        .map_err(internal)?
    {
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
        let ip = parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|info| info.0.ip().to_string());
        // An audit write failure is logged rather than failing the request:
        // the tracing line above still records the access.
        if let Err(err) = crate::services::admin_audit::record(
            &state.db,
            claims.sub,
            parts.method.as_str(),
            parts.uri.path(),
            ip,
        )
        .await
        {
            tracing::warn!(error = %err, admin_user_id = %claims.sub, "failed to write admin audit row");
        }
        Ok(AdminUser)
    }
}
