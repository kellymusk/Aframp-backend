use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::auth::extractor::Session;
use crate::auth::jwt;
use crate::auth::password;
use crate::error::{bad_request, bad_request_field, internal, unauthorized, ApiResult, ErrorCode};
use crate::models::{AuthResponse, LoginRequest, OtpChallengeResponse, SignupRequest, VerifyOtpRequest};
use crate::services::otp::{self, VerifiedOutcome};
use crate::services::users;
use crate::validation::{is_valid_email, normalize_ng_phone_number, validate_name, validate_password, MAX_PASSWORD_LEN};
use crate::AppState;

/// Validates and hashes the credentials, then hands off to an OTP challenge
/// — the account itself doesn't exist yet. It's only ever created inside
/// `verify_otp`, once the code is confirmed; see `services::otp::verify`.
pub async fn signup(
    State(state): State<AppState>,
    Json(req): Json<SignupRequest>,
) -> ApiResult<Json<OtpChallengeResponse>> {
    if !is_valid_email(&req.email) {
        return Err(bad_request_field("email", "must be a valid email address"));
    }
    // Argon2's cost scales with input size, so cap the password before it
    // reaches the hasher (defence in depth behind the request body limit).
    if req.password.len() > MAX_PASSWORD_LEN {
        return Err(bad_request_field("password", "must be at most 1024 characters"));
    }
    if let Err(errors) = validate_password(&req.password) {
        let message = format!("password {}", errors.join(", "));
        return Err(bad_request_field("password", &message));
    }
    let name = validate_name(&req.name).map_err(|msg| bad_request_field("name", msg))?;
    let phone_number = normalize_ng_phone_number(&req.phone_number)
        .map_err(|msg| bad_request_field("phone_number", msg))?;
    let password_hash = password::hash(&req.password).map_err(internal)?;

    let challenge = otp::start_signup_challenge(
        &state.db,
        state.otp_provider.as_ref(),
        state.otp_hmac_secret.as_str(),
        &req.email,
        &password_hash,
        &name,
        &phone_number,
    )
    .await?;

    Ok(Json(challenge))
}

/// Verifies the password as before; a phone-verified account then gets an
/// OTP challenge instead of a session. An account with no phone on file
/// (only possible pre-migration — every signup requires one now) logs in
/// exactly as it always has, untouched by this rollout.
pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Response> {
    if !is_valid_email(&req.email) {
        return Err(bad_request_field("email", "must be a valid email address"));
    }
    // Argon2's cost scales with input size, so cap the password before it
    // reaches the hasher (defence in depth behind the request body limit).
    if req.password.len() > MAX_PASSWORD_LEN {
        return Err(bad_request_field("password", "must be at most 1024 characters"));
    }
    let (user, merchant) = users::login(&state.db, &req.email, &req.password)
        .await?;

    if user.phone_number.is_some() {
        let challenge = otp::start_login_challenge(
            &state.db,
            state.otp_provider.as_ref(),
            state.otp_hmac_secret.as_str(),
            &user,
        )
        .await?;
        return Ok(Json(challenge).into_response());
    }

    let token = jwt::sign(
        &state.jwt_secret,
        user.id,
        merchant.as_ref().map(|m| m.id),
        user.is_admin,
    )
    .map_err(internal)?;
    let resp = authenticated(
        &state,
        AuthResponse {
            token,
            user_id: user.id,
            merchant_id: merchant.map(|m| m.id),
        },
    )?;
    Ok(resp.into_response())
}

/// The only place that ever issues a session — reached either from a
/// signup challenge (which also materializes the account, here) or a
/// login challenge (which doesn't; the account already exists).
pub async fn verify_otp(
    State(state): State<AppState>,
    Json(req): Json<VerifyOtpRequest>,
) -> ApiResult<impl IntoResponse> {
    let outcome = otp::verify(&state.db, state.otp_hmac_secret.as_str(), req.challenge_id, &req.code)
        .await?;

    let (user, merchant_id) = match outcome {
        VerifiedOutcome::Login(user) | VerifiedOutcome::PhoneChanged(user) => {
            let merchant = users::merchant_by_user(&state.db, user.id).await.map_err(internal)?;
            (user, merchant.map(|m| m.id))
        }
        VerifiedOutcome::Signup(user, merchant) => (user, Some(merchant.id)),
    };

    let token = jwt::sign(&state.jwt_secret, user.id, merchant_id, user.is_admin).map_err(internal)?;
    authenticated(
        &state,
        AuthResponse {
            token,
            user_id: user.id,
            merchant_id,
        },
    )
}

/// Drops the session cookie. Deliberately unauthenticated: a browser holding an
/// expired or malformed session still needs a way to clear it.
pub async fn logout(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let cookie = state.cookie.clear().map_err(internal)?;
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]))
}

/// Exchanges a valid, unexpired session token for a fresh one (also reset
/// as the session cookie), up to `jwt::MAX_SESSION_DAYS` after the original
/// login; after that the user has to log in again.
pub async fn refresh(State(state): State<AppState>, Session(claims): Session) -> ApiResult<impl IntoResponse> {
    let token = jwt::refresh(&state.jwt_secret, &claims).map_err(|err| match err {
        jwt::RefreshError::Signing => internal(err),
        _ => unauthorized(ErrorCode::InvalidCredentials, &err.to_string()),
    })?;
    authenticated(
        &state,
        AuthResponse {
            token,
            user_id: claims.sub,
            merchant_id: claims.merchant_id,
        },
    )
}

/// Sets the session cookie for browsers and echoes the token for API clients.
fn authenticated(state: &AppState, body: AuthResponse) -> ApiResult<impl IntoResponse> {
    let cookie = state.cookie.session(&body.token).map_err(internal)?;
    Ok(([(header::SET_COOKIE, cookie)], Json(body)))
}
