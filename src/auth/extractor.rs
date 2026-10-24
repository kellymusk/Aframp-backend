use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::Method;
use axum::http::StatusCode;
use axum::Json;
use axum_extra::headers::authorization::Bearer;
use axum_extra::headers::Authorization;
use axum_extra::TypedHeader;

use crate::auth::{cookie, jwt};
use crate::auth::jwt::Claims;
use crate::error::{forbidden, internal, ApiError, ErrorCode};
use crate::services::users;
use crate::AppState;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: uuid::Uuid,
    pub merchant_id: Option<uuid::Uuid>,
}

/// Multi-level admin role carried on the `admin_role` JWT claim. `None` means
/// the token has no admin role (legacy admin tokens) and is treated as
/// [`AdminRole::SuperAdmin`] for backward compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminRole {
    SuperAdmin,
    Support,
    Finance,
}

impl AdminRole {
    pub fn from_claim(claim: Option<&str>) -> Option<Self> {
        match claim {
            Some("super_admin") => Some(AdminRole::SuperAdmin),
            Some("support") => Some(AdminRole::Support),
            Some("finance") => Some(AdminRole::Finance),
            _ => None,
        }
    }

    /// Financial data (withdrawals, overview balances) is restricted to
    /// `super_admin` and `finance`.
    pub fn can_view_financials(self) -> bool {
        matches!(self, AdminRole::SuperAdmin | AdminRole::Finance)
    }

    /// PII-heavy data (user emails) is restricted to `super_admin` and
    /// `support`.
    pub fn can_view_pii(self) -> bool {
        matches!(self, AdminRole::SuperAdmin | AdminRole::Support)
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

/// Same session proof as [`AuthUser`], but additionally requires the `is_admin`
/// JWT claim. The claim is baked in at login and not re-checked against the
/// database, so revoking admin access takes up to [`jwt::TOKEN_TTL_HOURS`] to
/// take effect on outstanding tokens.
///
/// State-changing requests (anything other than `GET`/`HEAD`/`OPTIONS`) that
/// authenticate via the session cookie must also present the per-session CSRF
/// token in the `X-CSRF-Token` header. The token is derived from the session
/// token and the server secret, so it is stable for the life of the session and
/// can be embedded in the admin dashboard HTML as a meta tag. Bearer-token
/// clients are exempt: they are not subject to cross-site cookie riding.
/// Same session proof as [`AuthUser`], but additionally requires admin rights.
/// The `is_admin` JWT claim is only a cheap first filter: every admin request
/// also re-reads `users.is_admin`, so revoking admin access in the database
/// takes effect on the very next request rather than when the token expires.
#[derive(Debug, Clone)]
pub struct AdminUser {
    pub user_id: uuid::Uuid,
    pub role: AdminRole,
}

impl AdminUser {
    /// Reject the request unless the admin holds a role allowed to view
    /// financial data.
    pub fn require_financials(&self) -> Result<(), (StatusCode, Json<ApiError>)> {
        if self.role.can_view_financials() {
            Ok(())
        } else {
            Err(forbidden(ErrorCode::Forbidden, "insufficient admin role for financial data"))
        }
    }

    /// Reject the request unless the admin holds a role allowed to view PII.
    pub fn require_pii(&self) -> Result<(), (StatusCode, Json<ApiError>)> {
        if self.role.can_view_pii() {
            Ok(())
        } else {
            Err(forbidden(ErrorCode::Forbidden, "insufficient admin role for user PII"))
        }
    }
}

fn authenticate(parts: &Parts, state: &AppState) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    // API clients send a bearer token; browsers send the HttpOnly session
    // cookie, which JS on the page cannot read. Either proves the session.
    // `TypedHeader<Authorization<Bearer>>` handles case-insensitive scheme
    // matching (e.g. `bearer`), so we fall back to the cookie when the header
    // is absent or malformed.
    let bearer = TypedHeader::<Authorization<Bearer>>::from_request_parts(
        &mut Parts {
            method: parts.method.clone(),
            uri: parts.uri.clone(),
            version: parts.version,
            headers: parts.headers.clone(),
            extensions: Default::default(),
        },
        state,
    )
    .ok()
    .map(|TypedHeader(Authorization(bearer))| bearer.token().to_owned());

    let token = bearer
        .or_else(|| cookie::from_headers(&parts.headers))
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, Json(ApiError { code: ErrorCode::InvalidCredentials, error: "missing session cookie or bearer token".into(), field: None })))?;
    jwt::verify(&state.jwt_secret, &token)
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ApiError { code: ErrorCode::InvalidCredentials, error: "invalid or expired token".into(), field: None })))
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
    let claims = jwt::verify(&state.jwt_secret, token)
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ApiError { code: ErrorCode::InvalidCredentials, error: "invalid or expired token".into(), field: None })))?;
    // A token presented after `POST /logout` (or any other revocation) is
    // refused even though its signature and expiry still check out.
    if crate::auth::revocation::is_revoked(claims.jti) {
        tracing::warn!(jti = %claims.jti, user_id = %claims.sub, "revoked token presented");
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiError { code: ErrorCode::InvalidCredentials, error: "token has been revoked".into(), field: None }),
        ));
    }
    Ok(claims)
}

/// Verifies the token, that its account still exists and hasn't been
/// deleted, and that the session began after the last password change — so
/// deleting an account or changing its password revokes every token issued
/// before that point. Refreshed tokens keep their original `orig_iat`, so a
/// refresh can't launder a session past a password change.
async fn authenticate_active(
    parts: &Parts,
    state: &AppState,
) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    let claims = authenticate(parts, state)?;
    let session_start = if claims.orig_iat == 0 { claims.iat } else { claims.orig_iat } as i64;
    if !users::session_is_valid(&state.db, claims.sub, session_start)
        .await
        .map_err(internal)?
    {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiError { code: ErrorCode::InvalidCredentials, error: "invalid or expired token".into(), field: None }),
        ));
    }
    Ok(claims)
}

/// Derive the per-session CSRF token from the raw session token and the server
/// secret. Deterministic so the dashboard can render it once and reuse it for
/// every mutating request without extra server-side storage.
pub fn csrf_token(jwt_secret: &[u8], session_token: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(jwt_secret)
        .expect("HMAC accepts keys of any size");
    mac.update(b"admin-csrf-v1:");
    mac.update(session_token.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn is_state_changing(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = (StatusCode, Json<ApiError>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let claims = authenticate_active(parts, state).await?;
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
        let claims = authenticate_active(parts, state).await?;
        if !claims.is_admin {
            return Err(forbidden(ErrorCode::Forbidden, "admin access required"));
        }
        // Legacy admin tokens without an explicit role are treated as
        // super_admin so existing sessions keep working until they expire.
        let role = AdminRole::from_claim(claims.admin_role.as_deref())
            .unwrap_or(AdminRole::SuperAdmin);
        tracing::info!(admin_user_id = %claims.sub, admin_role = ?role, path = %parts.uri.path(), "admin access");
        Ok(AdminUser {
            user_id: claims.sub,
            role,
        })

        // CSRF protection for cookie-authenticated state-changing requests.
        // Bearer-token clients are exempt (no ambient cookie to ride).
        let via_bearer = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some();
        if !via_bearer && is_state_changing(&parts.method) {
            let session_token = cookie::from_headers(&parts.headers).ok_or_else(|| {
                forbidden(ErrorCode::Forbidden, "missing session cookie")
            })?;
            let expected = csrf_token(&state.jwt_secret, &session_token);
            let provided = parts
                .headers
                .get("x-csrf-token")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if !constant_time_eq(expected.as_bytes(), provided.as_bytes()) {
                return Err(forbidden(ErrorCode::Forbidden, "invalid or missing CSRF token"));
            }
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

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn parts_with_authorization(value: &str) -> Parts {
        let mut parts = Parts {
            method: axum::http::Method::GET,
            uri: "/".parse().unwrap(),
            version: axum::http::Version::HTTP_11,
            headers: axum::http::HeaderMap::new(),
            extensions: Default::default(),
        };
        parts
            .headers
            .insert("authorization", HeaderValue::from_str(value).unwrap());
        parts
    }

    fn extract_bearer(parts: &Parts) -> Option<String> {
        TypedHeader::<Authorization<Bearer>>::from_request_parts(
            &mut Parts {
                method: parts.method.clone(),
                uri: parts.uri.clone(),
                version: parts.version,
                headers: parts.headers.clone(),
                extensions: Default::default(),
            },
            &(),
        )
        .ok()
        .map(|TypedHeader(Authorization(bearer))| bearer.token().to_owned())
    }

    #[test]
    fn bearer_scheme_is_case_insensitive() {
        let parts = parts_with_authorization("bearer abc.def.ghi");
        assert_eq!(extract_bearer(&parts).as_deref(), Some("abc.def.ghi"));
    }

    #[test]
    fn bearer_scheme_uppercase() {
        let parts = parts_with_authorization("Bearer abc.def.ghi");
        assert_eq!(extract_bearer(&parts).as_deref(), Some("abc.def.ghi"));
    }
}
