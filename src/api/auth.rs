use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{header, Extensions, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::auth::jwt;
use crate::auth::password;
use crate::error::{
    bad_request, bad_request_field, conflict, internal, not_found, too_many_requests, unauthorized,
    ApiError, ApiResult, ErrorCode,
};
use crate::models::{AuthResponse, LoginRequest, OtpChallengeResponse, SignupRequest, VerifyOtpRequest};
use crate::services::login_rate_limit;
use crate::services::otp::{self, OtpError, VerifiedOutcome};
use crate::services::users::{self, UserError};
use crate::validation::{is_valid_email, normalize_ng_phone_number, validate_name};
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
    if req.password.len() < 8 {
        return Err(bad_request_field(
            "password",
            "must be at least 8 characters",
        ));
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
    .await
    .map_err(map_otp_error)?;

    Ok(Json(challenge))
}

/// Verifies the password as before; a phone-verified account then gets an
/// OTP challenge instead of a session. An account with no phone on file
/// (only possible pre-migration — every signup requires one now) logs in
/// exactly as it always has, untouched by this rollout.
///
/// Attempts are rate limited per client IP and per email address (see
/// `services::login_rate_limit`); over the limit returns `429` with
/// `Retry-After`. A successful password check resets the email's counter.
pub async fn login(
    State(state): State<AppState>,
    extensions: Extensions,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Response> {
    if !is_valid_email(&req.email) {
        return Err(bad_request_field("email", "must be a valid email address"));
    }

    let email_key = login_rate_limit::email_key(&req.email);
    let mut keys = vec![(email_key.clone(), login_rate_limit::EMAIL_LIMIT)];
    if let Some(ConnectInfo(addr)) = extensions.get::<ConnectInfo<SocketAddr>>() {
        keys.push((login_rate_limit::ip_key(addr.ip()), login_rate_limit::IP_LIMIT));
    }
    for (key, limit) in &keys {
        if let Some(retry_after) = login_rate_limit::hit(&state.db, key, *limit)
            .await
            .map_err(internal)?
        {
            return Ok(login_rate_limited(retry_after));
        }
    }

    let (user, merchant) = users::login(&state.db, &req.email, &req.password)
        .await
        .map_err(map_user_error)?;
    login_rate_limit::reset(&state.db, &email_key)
        .await
        .map_err(internal)?;

    if user.phone_number.is_some() {
        let challenge = otp::start_login_challenge(
            &state.db,
            state.otp_provider.as_ref(),
            state.otp_hmac_secret.as_str(),
            &user,
        )
        .await
        .map_err(map_otp_error)?;
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
        .await
        .map_err(map_otp_error)?;

    let (user, merchant_id) = match outcome {
        VerifiedOutcome::Login(user) => {
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

fn login_rate_limited(retry_after_secs: i64) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, retry_after_secs.to_string())],
        Json(ApiError {
            error: "too many login attempts, please try again later".into(),
            code: ErrorCode::TooManyRequests,
            field: None,
        }),
    )
        .into_response()
}

/// Sets the session cookie for browsers and echoes the token for API clients.
fn authenticated(state: &AppState, body: AuthResponse) -> ApiResult<impl IntoResponse> {
    let cookie = state.cookie.session(&body.token).map_err(internal)?;
    Ok(([(header::SET_COOKIE, cookie)], Json(body)))
}

fn map_user_error(err: UserError) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        UserError::InvalidCredentials => {
            unauthorized(ErrorCode::InvalidCredentials, "invalid email or password")
        }
        UserError::Database(_) => internal(err),
    }
}

fn map_otp_error(err: OtpError) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        OtpError::RateLimited => {
            too_many_requests(ErrorCode::TooManyRequests, "too many requests, please try again shortly")
        }
        OtpError::ChallengeNotFound => {
            not_found(ErrorCode::OtpChallengeNotFound, "otp challenge not found or already used")
        }
        OtpError::Expired => bad_request(ErrorCode::OtpExpired, "otp code has expired"),
        OtpError::Locked => bad_request(ErrorCode::OtpLocked, "too many incorrect attempts — request a new code"),
        OtpError::InvalidCode => bad_request(ErrorCode::OtpInvalid, "incorrect code"),
        OtpError::EmailTaken => conflict(ErrorCode::EmailTaken, "email already registered"),
        OtpError::PhoneTaken => conflict(ErrorCode::PhoneTaken, "phone number already registered"),
        OtpError::SendFailed(_) | OtpError::Database(_) => internal(err),
    }
}
