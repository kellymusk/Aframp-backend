use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use tokio::sync::Mutex;

use crate::auth::extractor::Session;
use crate::auth::jwt;
use crate::auth::password;
use crate::error::{
    bad_request, bad_request_field, internal,
    ApiResult,
};
use crate::models::{AuthResponse, LoginRequest, OtpChallengeResponse, SignupRequest, VerifyOtpRequest};
use crate::services::otp::{self, VerifiedOutcome};
use crate::services::users;
use crate::validation::{is_valid_email, normalize_ng_phone_number, validate_name};
use crate::AppState;

/// Maximum failed login attempts allowed per email within the window.
const LOGIN_MAX_ATTEMPTS: usize = 10;
/// Length of the sliding window over which attempts are counted.
const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);

/// In-memory sliding-window limiter keyed by email. Tracks the timestamps of
/// recent failed attempts; a successful login clears the entry. This is
/// process-local — a distributed deployment would need a shared store (Redis).
fn login_limiter() -> &'static Mutex<HashMap<String, Vec<Instant>>> {
    static LIMITER: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();
    LIMITER.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Returns `Ok(())` if the email is under the limit, or `Err(retry_after_secs)`
/// with the number of seconds until the oldest attempt falls out of the window.
async fn check_login_rate_limit(email: &str) -> Result<(), u64> {
    let now = Instant::now();
    let mut map = login_limiter().lock().await;
    let attempts = map.entry(email.to_string()).or_default();
    attempts.retain(|t| now.duration_since(*t) < LOGIN_WINDOW);
    if attempts.len() >= LOGIN_MAX_ATTEMPTS {
        let oldest = attempts.iter().min().copied().unwrap_or(now);
        let elapsed = now.duration_since(oldest);
        let retry_after = LOGIN_WINDOW.saturating_sub(elapsed).as_secs().max(1);
        return Err(retry_after);
    }
    Ok(())
}

/// Records a failed attempt for the email, pruning stale entries first.
async fn record_login_failure(email: &str) {
    let now = Instant::now();
    let mut map = login_limiter().lock().await;
    let attempts = map.entry(email.to_string()).or_default();
    attempts.retain(|t| now.duration_since(*t) < LOGIN_WINDOW);
    attempts.push(now);
}

/// Clears the failure history for an email after a successful login.
async fn clear_login_failures(email: &str) {
    let mut map = login_limiter().lock().await;
    map.remove(email);
}

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

    if let Err(retry_after) = check_login_rate_limit(&req.email).await {
        return Err(rate_limited(retry_after));
    }

    let (user, merchant) = match users::login(&state.db, &req.email, &req.password).await {
        Ok(pair) => pair,
        Err(err) => {
            if matches!(err, UserError::InvalidCredentials) {
                record_login_failure(&req.email).await;
            }
            return Err(map_user_error(err));
        }
    };
    clear_login_failures(&req.email).await;
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

/// Builds a `429 Too Many Requests` response carrying a `Retry-After` header.
fn rate_limited(retry_after_secs: u64) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    let (status, body) = too_many_requests(
        ErrorCode::TooManyRequests,
        "too many login attempts, please try again later",
    );
    (status, body)
}

fn map_user_error(err: UserError) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        UserError::InvalidCredentials => {
            unauthorized(ErrorCode::InvalidCredentials, "invalid email or password")
        }
        UserError::AccountLocked { until } => {
            let msg = format!(
                "account locked due to too many failed login attempts; try again after {}",
                until.format("%Y-%m-%dT%H:%M:%SZ")
            );
            crate::error::forbidden(ErrorCode::Forbidden, &msg)
        }
        UserError::MerchantSuspended => {
            crate::error::forbidden(ErrorCode::Forbidden, "this merchant account has been suspended")
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
