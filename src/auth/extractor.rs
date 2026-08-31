use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::Json;

use crate::auth::{cookie, jwt};
use crate::error::ApiError;
use crate::services::api_keys;
use crate::AppState;

fn unauthorized(message: &str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiError {
            error: message.into(),
            field: None,
        }),
    )
}

/// How the caller proved who they are. Most handlers don't care, but a few
/// must: minting an API key is JWT-only, so a leaked key cannot be used to
/// mint a fresh one and outlive its own revocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// A session JWT, from the `Authorization` header or the session cookie.
    Session,
    /// A long-lived `sk_`-prefixed API key.
    ApiKey,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: uuid::Uuid,
    pub merchant_id: Option<uuid::Uuid>,
    pub via: AuthMethod,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = (StatusCode, Json<ApiError>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // API clients send a bearer token; browsers send the HttpOnly session
        // cookie, which JS on the page cannot read. Either proves the session.
        let bearer = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));

        // Two kinds of bearer travel on this header. An `sk_`-prefixed value is
        // a long-lived API key for server-to-server callers; anything else is a
        // JWT. Shape alone decides, so a malformed key never gets fed to the
        // JWT verifier and vice versa.
        if let Some(presented) = bearer.filter(|v| v.starts_with("sk_")) {
            let principal = api_keys::authenticate(&state.db, presented)
                .await
                .map_err(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiError {
                            error: "internal server error".into(),
                            field: None,
                        }),
                    )
                })?
                .ok_or_else(|| unauthorized("invalid or revoked api key"))?;
            return Ok(AuthUser {
                user_id: principal.user_id,
                merchant_id: Some(principal.merchant_id),
                via: AuthMethod::ApiKey,
            });
        }

        let token = bearer
            .or_else(|| cookie::from_headers(&parts.headers))
            .ok_or_else(|| unauthorized("missing session cookie or bearer token"))?;
        let claims = jwt::verify(&state.jwt_secret, token)
            .map_err(|_| unauthorized("invalid or expired token"))?;
        Ok(AuthUser {
            user_id: claims.sub,
            merchant_id: claims.merchant_id,
            via: AuthMethod::Session,
        })
    }
}
