use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::jwt::{
    blacklist_access_token, generate_access_token, generate_refresh_token, revoke_refresh_token,
    store_refresh_token, validate_token, JwtError, RefreshTokenRecord, Scope, TokenType,
};
use crate::cache::RedisCache;

#[derive(Clone)]
pub struct AuthHandlerState {
    pub jwt_secret: String,
    pub redis_cache: Option<RedisCache>,
}

// ── Request / Response types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    pub wallet_address: String,
    pub signature: String,
    pub message: String,
    pub timestamp: i64,
}

#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: &'static str,
    pub expires_in: i64,
    pub scope: String,
    pub wallet_address: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Serialize)]
pub struct RefreshResponse {
    pub access_token: String,
}

#[derive(Debug, Deserialize)]
pub struct GenerateTokenRequest {
    pub wallet_address: String,
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RefreshTokenRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub struct RevokeTokenRequest {
    pub refresh_token: Option<String>,
    pub access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RevokeRequest {
    pub token: Option<String>,
    #[serde(default)]
    pub revoke_all: bool,
}

#[derive(Debug, Serialize)]
pub struct RevokeResponse {
    pub revoked: bool,
    pub message: &'static str,
}

// ── Error helper ──────────────────────────────────────────────────────────────

fn auth_error(status: u16, code: &'static str, message: &'static str) -> axum::response::Response {
    use axum::http::StatusCode;
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        Json(serde_json::json!({ "error": { "code": code, "message": message } })),
    )
        .into_response()
}


// ── Scope resolution ──────────────────────────────────────────────────────────

fn resolve_scope(wallet_address: &str) -> Scope {
    // Admin wallets are listed in JWT_ADMIN_WALLETS (comma-separated)
    if let Ok(admins) = std::env::var("JWT_ADMIN_WALLETS") {
        if admins.split(',').any(|w| w.trim() == wallet_address) {
            return Scope::Admin;
        }
    }
    Scope::User
}

// ── POST /api/auth/token ──────────────────────────────────────────────────────

pub async fn generate_token(
    State(state): State<Arc<AuthHandlerState>>,
    headers: HeaderMap,
    Json(payload): Json<TokenRequest>,
) -> axum::response::Response {
    let _request_id = get_request_id_from_headers(&headers);

    if payload.wallet_address.trim().is_empty() {
        return auth_error(400, "VALIDATION_ERROR", "wallet_address is required");
    }
    if payload.signature.trim().is_empty() {
        return auth_error(400, "VALIDATION_ERROR", "signature is required");
    }
    if payload.message.trim().is_empty() {
        return auth_error(400, "VALIDATION_ERROR", "message is required");
    }

    // Reject stale requests (> 5 minutes)
    let now = Utc::now().timestamp();
    if (now - payload.timestamp).abs() > 300 {
        return auth_error(400, "VALIDATION_ERROR", "timestamp is too old or in the future");
    }

    // Verify wallet signature
    match super::jwt::verify_wallet_signature(
        &payload.wallet_address,
        &payload.message,
        &payload.signature,
    ) {
        Ok(true) => {}
        Ok(false) => {
            return auth_error(401, "INVALID_SIGNATURE", "Wallet signature verification failed");
        }
        Err(_) => {
            return auth_error(400, "INVALID_WALLET", "Invalid wallet address or signature format");
        }
    }

    let scope = resolve_scope(&payload.wallet_address);

    let (access_token, _) =
        match generate_access_token(&payload.wallet_address, scope.clone(), &state.jwt_secret) {
            Ok(t) => t,
            Err(_) => return auth_error(500, "INTERNAL_ERROR", "Failed to generate access token"),
        };

    let (refresh_token_str, refresh_claims) =
        match generate_refresh_token(&payload.wallet_address, scope.clone(), &state.jwt_secret) {
            Ok(t) => t,
            Err(_) => return auth_error(500, "INTERNAL_ERROR", "Failed to generate refresh token"),
        };

    if let Some(cache) = &state.redis_cache {
        let jti = refresh_claims.jti.as_deref().unwrap_or("");
        let record = RefreshTokenRecord {
            wallet_address: payload.wallet_address.clone(),
            issued_at: refresh_claims.iat,
            expires_at: refresh_claims.exp,
        };
        if let Err(e) = store_refresh_token(cache, jti, &record).await {
            tracing::error!(error = %e, "Failed to store refresh token in Redis");
            return auth_error(500, "INTERNAL_ERROR", "Failed to persist session");
        }
    }

    Json(TokenResponse {
        access_token,
        refresh_token: refresh_token_str,
        token_type: "Bearer",
        expires_in: super::jwt::ACCESS_TOKEN_TTL_SECS,
        scope: scope.to_string(),
        wallet_address: payload.wallet_address,
    })
    .into_response()
}

// ── POST /api/auth/refresh ────────────────────────────────────────────────────

pub async fn refresh_token(
    State(state): State<Arc<AuthHandlerState>>,
    Json(payload): Json<RefreshRequest>,
) -> axum::response::Response {
    if payload.refresh_token.trim().is_empty() {
        return auth_error(400, "VALIDATION_ERROR", "refresh_token is required");
    }

    let claims = match validate_token(&payload.refresh_token, &state.jwt_secret) {
        Ok(c) => c,
        Err(JwtError::TokenExpired) => {
            return auth_error(401, "TOKEN_EXPIRED", "Refresh token has expired. Please re-authenticate.");
        }
        Err(_) => {
            return auth_error(401, "INVALID_TOKEN", "Invalid refresh token");
        }
    };

    // Must be a refresh token
    if claims.token_type != TokenType::Refresh {
        return auth_error(400, "INVALID_TOKEN", "Provided token is not a refresh token");
    }

    // Check revocation in Redis
    if let Some(cache) = &state.redis_cache {
        let jti = match &claims.jti {
            Some(j) => j.clone(),
            None => return auth_error(400, "INVALID_TOKEN", "Refresh token missing JTI"),
        };
        match is_refresh_token_valid(cache, &jti).await {
            Ok(false) => return auth_error(401, "TOKEN_REVOKED", "Refresh token has been revoked"),
            Err(_) => {} // graceful degradation
            Ok(true) => {}
        }

        // Rotate: revoke old refresh token
        let _ = revoke_refresh_token(cache, &jti).await;
    }

    // Issue new access token
    let (new_access_token, _) =
        match generate_access_token(&claims.sub, claims.scope.clone(), &state.jwt_secret) {
            Ok(t) => t,
            Err(_) => return auth_error(500, "INTERNAL_ERROR", "Failed to generate access token"),
        };

    // Issue new refresh token (rotation)
    if let Some(cache) = &state.redis_cache {
        let (new_refresh_str, new_refresh_claims) =
            match generate_refresh_token(&claims.sub, claims.scope.clone(), &state.jwt_secret) {
                Ok(t) => t,
                Err(_) => return auth_error(500, "INTERNAL_ERROR", "Failed to generate refresh token"),
            };
        let jti = new_refresh_claims.jti.as_deref().unwrap_or("");
        let record = RefreshTokenRecord {
            wallet_address: claims.sub.clone(),
            issued_at: new_refresh_claims.iat,
            expires_at: new_refresh_claims.exp,
        };
        let _ = store_refresh_token(cache, jti, &record).await;
        // Return both tokens when rotation is enabled
        return Json(serde_json::json!({
            "access_token": new_access_token,
            "refresh_token": new_refresh_str,
            "token_type": "Bearer",
            "expires_in": super::jwt::ACCESS_TOKEN_TTL_SECS,
        }))
        .into_response();
    }

    Json(RefreshResponse {
        access_token: new_access_token,
        token_type: "Bearer",
        expires_in: super::jwt::ACCESS_TOKEN_TTL_SECS,
    })
    .into_response()
}

// ── POST /api/auth/revoke ─────────────────────────────────────────────────────

pub async fn revoke_token(
    State(state): State<Arc<AuthHandlerState>>,
    headers: HeaderMap,
    Json(payload): Json<RevokeRequest>,
) -> axum::response::Response {
    // Extract the caller's access token to identify the wallet
    let caller_token = match headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        Some(t) => t.to_string(),
        None => return auth_error(401, "MISSING_TOKEN", "Authentication token required"),
    };

    let caller_claims = match validate_token(&caller_token, &state.jwt_secret) {
        Ok(c) => c,
        Err(JwtError::TokenExpired) => {
            return auth_error(401, "TOKEN_EXPIRED", "Access token has expired");
        }
        Err(_) => return auth_error(401, "INVALID_TOKEN", "Invalid access token"),
    };

    let cache = match &state.redis_cache {
        Some(c) => c,
        None => {
            // No Redis – nothing to revoke, return success
            return Json(RevokeResponse {
                revoked: true,
                message: "Token revoked successfully",
            })
            .into_response();
        }
    };

    if payload.revoke_all {
        // Revoke all sessions for this wallet
        let _ = super::jwt::revoke_all_sessions(cache, &caller_claims.sub).await;
        return Json(RevokeResponse {
            revoked: true,
            message: "All sessions revoked successfully",
        })
        .into_response();
    }

    // Revoke the specific refresh token provided in the body
    let token_str = match &payload.token {
        Some(t) => t.clone(),
        None => return auth_error(400, "VALIDATION_ERROR", "token is required when revoke_all is false"),
    };

    let token_claims = match validate_token(&token_str, &state.jwt_secret) {
        Ok(c) => c,
        Err(_) => return auth_error(400, "INVALID_TOKEN", "Provided token is invalid"),
    };

    // Ensure the caller owns this token
    if token_claims.sub != caller_claims.sub {
        return auth_error(403, "FORBIDDEN", "Cannot revoke a token belonging to another wallet");
    }

    if token_claims.token_type != TokenType::Refresh {
        return auth_error(400, "INVALID_TOKEN", "Only refresh tokens can be explicitly revoked");
    }

    let jti = match &token_claims.jti {
        Some(j) => j.clone(),
        None => return auth_error(400, "INVALID_TOKEN", "Token missing JTI"),
    };

    match revoke_refresh_token(cache, &jti).await {
        Ok(_) => Json(RevokeResponse {
            revoked: true,
            message: "Token revoked successfully",
        })
        .into_response(),
        Err(_) => auth_error(500, "INTERNAL_ERROR", "Failed to revoke token"),
    }
}

fn parse_scope(scope: Option<&str>) -> Scope {
    match scope.unwrap_or("user") {
        "admin" => Scope::Admin,
        _ => Scope::User,
    }
}

fn jwt_error_response(err: JwtError) -> (StatusCode, Json<serde_json::Value>) {
    let status = match err {
        JwtError::MissingToken | JwtError::InvalidToken | JwtError::TokenExpired | JwtError::TokenRevoked => StatusCode::UNAUTHORIZED,
        JwtError::InsufficientPermissions { .. } => StatusCode::FORBIDDEN,
        JwtError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };

    (
        status,
        Json(json!({
            "error": err.to_string(),
        })),
    )
}
