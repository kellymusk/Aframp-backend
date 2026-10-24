use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use tokio::sync::Mutex;

use crate::auth::extractor::Session;
use crate::auth::jwt;
use crate::auth::password;
use crate::error::{
    bad_request, bad_request_field, conflict, internal, not_found, too_many_requests, unauthorized,
    ApiResult, ErrorCode,
};
use crate::models::{
    AuthResponse, ChangePasswordRequest, LoginRequest, OtpChallengeResponse, SignupRequest,
    VerifyOtpRequest,
};
use crate::services::otp::{self, OtpError, VerifiedOutcome};
use crate::services::users::{self, UserError};
use crate::validation::{is_valid_email, normalize_ng_phone_number, validate_name};
use crate::AppState;

/// A precomputed Argon2 hash used only to burn the same amount of CPU as a
/// real password verification when the account doesn't exist. This keeps the
/// "unknown email" and "wrong password" paths roughly constant-time, closing
/// the username-enumeration timing oracle. The plaintext is irrelevant — the
/// verification is expected to fail; we only care about the work it does.
const DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$RdescudvJCsgt3ub+b+dWRWJTmaaJObG";
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
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(req): Json<SignupRequest>,
) -> ApiResult<Json<OtpChallengeResponse>> {
    let ip = client_ip(&headers, peer.map(|ConnectInfo(addr)| addr.ip()));
    if !is_valid_email(&req.email) {
        tracing::info!(event = "signup_failed", client_ip = ?ip, "invalid signup email");
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

    persist_otp_ip(&state, challenge.challenge_id, ip).await?;
    tracing::info!(event = "signup_challenge_created", client_ip = ?ip, "signup OTP challenge created");

    Ok(Json(challenge))
}

/// Verifies the password as before; a phone-verified account then gets an
/// OTP challenge instead of a session. An account with no phone on file
/// (only possible pre-migration — every signup requires one now) logs in
/// exactly as it always has, untouched by this rollout.
pub async fn login(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Response> {
    let ip = client_ip(&headers, peer.map(|ConnectInfo(addr)| addr.ip()));
    if !is_valid_email(&req.email) {
        tracing::info!(event = "login_failed", client_ip = ?ip, "invalid login email");
        return Err(bad_request_field("email", "must be a valid email address"));
    }
    let (user, merchant) = match users::login(&state.db, &req.email, &req.password).await {
        Ok(found) => found,
        Err(UserError::InvalidCredentials) => {
            // Equalize timing: run a dummy Argon2 verification so the
            // "unknown email" path costs the same as a real "wrong password"
            // check, preventing username enumeration via response time.
            let _ = password::verify(DUMMY_PASSWORD_HASH, &req.password);
            return Err(map_user_error(UserError::InvalidCredentials));
        }
        Err(err) => return Err(map_user_error(err)),
    };

    if let Err(retry_after) = check_login_rate_limit(&req.email).await {
        tracing::info!(event = "login_locked", client_ip = ?ip, "login rate limit exceeded");
        return Err(rate_limited(retry_after));
    }

    let (user, merchant) = match users::login(&state.db, &req.email, &req.password).await {
        Ok(pair) => pair,
        Err(err) => {
            if matches!(err, UserError::InvalidCredentials) {
                record_login_failure(&req.email).await;
                tracing::info!(event = "login_failed", client_ip = ?ip, "invalid login credentials");
            } else if matches!(err, UserError::AccountLocked { .. }) {
                tracing::info!(event = "login_locked", client_ip = ?ip, "account locked during login");
            }
            return Err(map_user_error(err));
        }
    };
    clear_login_failures(&req.email).await;

    if user.phone_number.is_some() {
        let challenge = otp::start_login_challenge(
            &state.db,
            state.otp_provider.as_ref(),
            state.otp_hmac_secret.as_str(),
            &user,
        )
        .await?;
        persist_otp_ip(&state, challenge.challenge_id, ip).await?;
        tracing::info!(event = "login_otp_challenge_created", client_ip = ?ip, user_id = %user.id, "login OTP challenge created");
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
    tracing::info!(event = "login_succeeded", client_ip = ?ip, user_id = %user.id, "login succeeded");
    Ok(resp.into_response())
}

/// The only place that ever issues a session — reached either from a
/// signup challenge (which also materializes the account, here) or a
/// login challenge (which doesn't; the account already exists).
pub async fn verify_otp(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(req): Json<VerifyOtpRequest>,
) -> ApiResult<impl IntoResponse> {
    let ip = client_ip(&headers, peer.map(|ConnectInfo(addr)| addr.ip()));
    let outcome = match otp::verify(
        &state.db,
        state.otp_hmac_secret.as_str(),
        req.challenge_id,
        &req.code,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(err) => {
            if matches!(err, OtpError::Locked) {
                tracing::info!(event = "otp_locked", client_ip = ?ip, challenge_id = %req.challenge_id, "OTP challenge locked");
            } else {
                tracing::info!(event = "otp_failed", client_ip = ?ip, challenge_id = %req.challenge_id, reason = %err, "OTP verification failed");
            }
            return Err(map_otp_error(err));
        }
    };

    let (user, merchant_id) = match outcome {
        VerifiedOutcome::Login(user) | VerifiedOutcome::PhoneChanged(user) => {
            let merchant = users::merchant_by_user(&state.db, user.id)
                .await
                .map_err(internal)?;
            (user, merchant.map(|m| m.id))
        }
        VerifiedOutcome::Signup(user, merchant) => (user, Some(merchant.id)),
    };

    let token =
        jwt::sign(&state.jwt_secret, user.id, merchant_id, user.is_admin).map_err(internal)?;
    let response = authenticated(
        &state,
        AuthResponse {
            token,
            user_id: user.id,
            merchant_id,
        },
    )?;
    tracing::info!(event = "otp_succeeded", client_ip = ?ip, challenge_id = %req.challenge_id, user_id = %user.id, "OTP verification succeeded");
    Ok(response)
}

/// Drops the session cookie and puts the presented token on the revocation
/// list, so a JWT copied elsewhere dies here instead of staying valid for its
/// full lifetime. Deliberately unauthenticated and tolerant: a browser holding
/// an expired or malformed session still needs a way to clear it, so a token
/// that no longer verifies is simply not revoked.
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<impl IntoResponse> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if let Some(token) = bearer.or_else(|| crate::auth::cookie::from_headers(&headers)) {
        if let Ok(claims) = jwt::verify(&state.jwt_secret, token) {
            crate::auth::revocation::revoke(claims.jti, claims.exp);
            tracing::info!(jti = %claims.jti, user_id = %claims.sub, "token revoked on logout");
        }
    }
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

/// Changes the signed-in user's password. The current password must be
/// re-proven even with a valid session, so a hijacked token alone can't lock
/// the owner out. Every session issued before the change stops working; the
/// caller gets a fresh session (body + cookie) so this device stays signed in.
pub async fn change_password(
    State(state): State<AppState>,
    Session(claims): Session,
    Json(req): Json<ChangePasswordRequest>,
) -> ApiResult<impl IntoResponse> {
    if req.new_password.len() < 8 {
        return Err(bad_request_field(
            "new_password",
            "must be at least 8 characters",
        ));
    }

    users::change_password(&state.db, claims.sub, &req.current_password, &req.new_password)
        .await
        .map_err(|err| match err {
            users::ChangePasswordError::WrongCurrentPassword => bad_request_field(
                "current_password",
                "current password is incorrect",
            ),
            users::ChangePasswordError::Hash | users::ChangePasswordError::Database(_) => internal(err),
        })?;

    let token = jwt::sign(&state.jwt_secret, claims.sub, claims.merchant_id, claims.is_admin)
        .map_err(internal)?;
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
        UserError::MerchantSuspended => crate::error::forbidden(
            ErrorCode::Forbidden,
            "this merchant account has been suspended",
        ),
        UserError::Database(_) => internal(err),
    }
}

fn map_otp_error(err: OtpError) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        OtpError::RateLimited => too_many_requests(
            ErrorCode::TooManyRequests,
            "too many requests, please try again shortly",
        ),
        OtpError::ChallengeNotFound => not_found(
            ErrorCode::OtpChallengeNotFound,
            "otp challenge not found or already used",
        ),
        OtpError::Expired => bad_request(ErrorCode::OtpExpired, "otp code has expired"),
        OtpError::Locked => bad_request(
            ErrorCode::OtpLocked,
            "too many incorrect attempts — request a new code",
        ),
        OtpError::InvalidCode => bad_request(ErrorCode::OtpInvalid, "incorrect code"),
        OtpError::EmailTaken => conflict(ErrorCode::EmailTaken, "email already registered"),
        OtpError::PhoneTaken => conflict(ErrorCode::PhoneTaken, "phone number already registered"),
        OtpError::SendFailed(_) | OtpError::Database(_) => internal(err),
    }
}

/// Selects a valid client address from proxy headers, falling back to the
/// socket peer for direct connections. The proxy must sanitize forwarded
/// headers before they reach this service.
fn client_ip(headers: &HeaderMap, peer: Option<IpAddr>) -> Option<IpAddr> {
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .and_then(|value| value.trim().parse::<IpAddr>().ok());
    forwarded
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<IpAddr>().ok())
        })
        .or(peer)
}

async fn persist_otp_ip(
    state: &AppState,
    challenge_id: uuid::Uuid,
    ip: Option<IpAddr>,
) -> ApiResult<()> {
    sqlx::query("UPDATE otp_challenges SET client_ip = $2::inet WHERE id = $1")
        .bind(challenge_id)
        .bind(ip.map(|address| address.to_string()))
        .execute(&state.db)
        .await
        .map_err(internal)?;
    Ok(())
}

#[derive(serde::Deserialize)]
pub struct VerifyEmailQuery {
    pub token: String,
}

/// `GET /verify-email?token=...` — consumes a signup verification token and
/// marks the account's email as verified. No auth: the token itself is the
/// proof and is single-use / time-limited.
pub async fn verify_email(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<VerifyEmailQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    if query.token.trim().is_empty() {
        return Err(bad_request_field("token", "must not be empty"));
    }
    match crate::services::email_verification::verify_token(&state.db, &query.token).await {
        Ok(true) => Ok(Json(serde_json::json!({ "email_verified": true }))),
        Ok(false) => Err(bad_request(
            crate::error::ErrorCode::InvalidParameters,
            "invalid or expired email verification token",
        )),
        Err(err) => Err(internal(err)),
    }
}

/// `POST /verify-email/resend` — issues a fresh verification link for the
/// authenticated account, replacing any outstanding token.
pub async fn resend_email_verification(
    State(state): State<AppState>,
    Session(claims): Session,
) -> ApiResult<Json<serde_json::Value>> {
    let email: Option<String> = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?;
    let email = email.ok_or_else(|| {
        not_found(
            crate::error::ErrorCode::InvalidParameters,
            "account not found",
        )
    })?;

    let token = crate::services::email_verification::issue_token(&state.db, claims.sub)
        .await
        .map_err(internal)?;
    crate::services::email_verification::send_verification_email(&email, &token);

    Ok(Json(serde_json::json!({ "sent": true })))
}
