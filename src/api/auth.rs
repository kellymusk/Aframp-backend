use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;

use std::collections::BTreeMap;

use crate::auth::jwt;
use crate::error::{bad_request, bad_request_with_fields, internal, ApiResult};
use crate::models::{AuthResponse, LoginRequest, SignupRequest};
use crate::services::users::{self, UserError};
use crate::AppState;

fn is_valid_email(email: &str) -> bool {
    let email = email.trim();
    if email.is_empty() || email != email.trim() || email.contains(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    if local.is_empty() || domain.is_empty() || local.starts_with('.') || local.ends_with('.') {
        return false;
    }
    if domain.starts_with('.') || domain.ends_with('.') || !domain.contains('.') {
        return false;
    }
    let domain_parts: Vec<_> = domain.split('.').collect();
    if domain_parts.iter().any(|part| part.is_empty() || part.len() < 2) {
        return false;
    }
    !local.contains("..")
}

pub async fn signup(
    State(state): State<AppState>,
    Json(req): Json<SignupRequest>,
) -> ApiResult<impl IntoResponse> {
    let mut field_errors = BTreeMap::new();
    if !is_valid_email(&req.email) {
        field_errors.insert("email".into(), "must be a valid email address".into());
    }
    if req.password.len() < 8 {
        field_errors.insert("password".into(), "must be at least 8 characters long".into());
    }
    if req.name.trim().is_empty() {
        field_errors.insert("name".into(), "is required".into());
    }
    if !field_errors.is_empty() {
        return Err(bad_request_with_fields("validation failed", field_errors));
    }
    let (user, merchant) = users::signup(&state.db, &req.email, &req.password, &req.name)
        .await
        .map_err(map_user_error)?;
    let token = jwt::sign(&state.jwt_secret, user.id, Some(merchant.id))
        .map_err(internal)?;
    authenticated(
        &state,
        AuthResponse {
            token,
            user_id: user.id,
            merchant_id: Some(merchant.id),
        },
    )
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> ApiResult<impl IntoResponse> {
    let mut field_errors = BTreeMap::new();
    if !is_valid_email(&req.email) {
        field_errors.insert("email".into(), "must be a valid email address".into());
    }
    if req.password.is_empty() {
        field_errors.insert("password".into(), "is required".into());
    }
    if !field_errors.is_empty() {
        return Err(bad_request_with_fields("validation failed", field_errors));
    }
    let (user, merchant) = users::login(&state.db, &req.email, &req.password)
        .await
        .map_err(map_user_error)?;
    let token = jwt::sign(&state.jwt_secret, user.id, merchant.as_ref().map(|m| m.id))
        .map_err(internal)?;
    authenticated(
        &state,
        AuthResponse {
            token,
            user_id: user.id,
            merchant_id: merchant.map(|m| m.id),
        },
    )
}

/// Drops the session cookie. Deliberately unauthenticated: a browser holding an
/// expired or malformed session still needs a way to clear it.
pub async fn logout(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let cookie = state.cookie.clear().map_err(internal)?;
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]))
}

/// Sets the session cookie for browsers and echoes the token for API clients.
fn authenticated(state: &AppState, body: AuthResponse) -> ApiResult<impl IntoResponse> {
    let cookie = state.cookie.session(&body.token).map_err(internal)?;
    Ok(([(header::SET_COOKIE, cookie)], Json(body)))
}

fn map_user_error(err: UserError) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        UserError::EmailTaken => crate::error::conflict("email already registered"),
        UserError::InvalidCredentials => crate::error::unauthorized("invalid email or password"),
        _ => internal(err),
    }
}
