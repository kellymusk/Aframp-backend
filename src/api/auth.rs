
use axum::extract::State;
use axum::http::{header, Extensions, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::auth::extractor::Session;
use crate::auth::jwt;
use crate::auth::password;
use crate::error::{bad_request_field, internal, unauthorized, ApiError, ApiResult, ErrorCode};
use crate::models::{AuthResponse, ChangePasswordRequest, LoginRequest, OtpChallengeResponse, SignupRequest, VerifyOtpRequest};
use crate::services::login_rate_limit;
use crate::services::otp::{self, VerifiedOutcome};
use crate::services::users;
use crate::validation::{is_valid_email, normalize_ng_phone_number, validate_name, validate_password, MAX_PASSWORD_LEN};
use crate::client_ip::client_ip;
use crate::AppState;

/// Validates and hashes the credentials, then hands off to an OTP challenge
/// — the account itself doesn't exist yet. It's only ever created inside
/// `verify_otp`, once the code is confirmed; see `services::otp::verify`.
pub async fn signup(
    State(state): State<AppState>,
    headers: HeaderMap,
    extensions: Extensions,
    Json(req): Json<SignupRequest>,
) -> ApiResult<Json<OtpChallengeResponse>> {
    let ip = client_ip(&headers, &extensions, state.trust_proxy_headers);
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
    record_challenge_ip(&state, challenge.challenge_id, ip).await;
    tracing::info!(event = "signup_challenge_created", client_ip = ?ip, "signup OTP challenge created");

    Ok(Json(challenge))
}

/// Verifies the password as before; a phone-verified account then gets an
/// OTP challenge instead of a session. An account with no phone on file
/// (only possible pre-migration — every signup requires one now) logs in
/// exactly as it always has, untouched by this rollout.
///
/// Attempts are rate limited per client IP (see `services::login_rate_limit`);
/// over the limit returns `429` with `Retry-After`. Per-account brute force
/// is handled by the failed-login lockout in `services::users::login`.
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    extensions: Extensions,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Response> {
    let ip = client_ip(&headers, &extensions, state.trust_proxy_headers);
    if let Some(ip) = ip {
        let key = login_rate_limit::ip_key(ip);
        if let Some(retry_after) = login_rate_limit::hit(&state.db, &key, login_rate_limit::IP_LIMIT)
            .await
            .map_err(internal)?
        {
            tracing::info!(event = "login_rate_limited", client_ip = %ip, "login rate limit exceeded");
            return Ok(login_rate_limited(retry_after));
        }
    }

    if !is_valid_email(&req.email) {
        return Err(bad_request_field("email", "must be a valid email address"));
    }
    // Argon2's cost scales with input size, so cap the password before it
    // reaches the hasher (defence in depth behind the request body limit).
    if req.password.len() > MAX_PASSWORD_LEN {
        return Err(bad_request_field("password", "must be at most 1024 characters"));
    }
    let (user, merchant) = match users::login(&state.db, &req.email, &req.password).await {
        Ok(found) => found,
        Err(err) => {
            tracing::info!(event = "login_failed", client_ip = ?ip, reason = %err, "login failed");
            return Err(err.into());
        }
    };

    if user.phone_number.is_some() {
        let challenge = otp::start_login_challenge(
            &state.db,
            state.otp_provider.as_ref(),
            state.otp_hmac_secret.as_str(),
            &user,
        )
        .await?;
        record_challenge_ip(&state, challenge.challenge_id, ip).await;
        tracing::info!(event = "login_challenge_created", client_ip = ?ip, user_id = %user.id, "login OTP challenge created");
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
    headers: HeaderMap,
    extensions: Extensions,
    Json(req): Json<VerifyOtpRequest>,
) -> ApiResult<impl IntoResponse> {
    let ip = client_ip(&headers, &extensions, state.trust_proxy_headers);
    let outcome = match otp::verify(&state.db, state.otp_hmac_secret.as_str(), req.challenge_id, &req.code).await {
        Ok(outcome) => outcome,
        Err(err) => {
            tracing::info!(event = "otp_failed", client_ip = ?ip, challenge_id = %req.challenge_id, reason = %err, "OTP verification failed");
            return Err(err.into());
        }
    };
    tracing::info!(event = "otp_verified", client_ip = ?ip, challenge_id = %req.challenge_id, "OTP verified");

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
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<impl IntoResponse> {
    // Revoke the presented token too, so a copy held elsewhere stops working.
    if let Some(token) = crate::auth::extractor::session_token(&headers) {
        if let Ok(claims) = jwt::verify(&state.jwt_secret, token) {
            jwt::revoke(&state.db, &claims).await.map_err(internal)?;
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
    if req.new_password.len() > MAX_PASSWORD_LEN {
        return Err(bad_request_field("new_password", "must be at most 1024 characters"));
    }
    if let Err(errors) = validate_password(&req.new_password) {
        let message = format!("password {}", errors.join(", "));
        return Err(bad_request_field("new_password", &message));
    }

    users::change_password(&state.db, claims.sub, &req.current_password, &req.new_password)
        .await
        .map_err(|err| match err {
            users::ChangePasswordError::WrongCurrentPassword => {
                bad_request_field("current_password", "current password is incorrect")
            }
            users::ChangePasswordError::Hash | users::ChangePasswordError::Database(_) => internal(err),
        })?;

    let token = jwt::sign(&state.jwt_secret, claims.sub, claims.merchant_id, claims.is_admin).map_err(internal)?;
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

fn login_rate_limited(retry_after_secs: i64) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, retry_after_secs.to_string())],
        Json(ApiError {
            error: "too many login attempts, please try again later".into(),
            code: ErrorCode::TooManyRequests,
            field: None,
            retry_after_secs: Some(retry_after_secs.max(0) as u64),
        }),
    )
        .into_response()
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
        Ok(false) => Err(crate::error::bad_request(
            ErrorCode::InvalidParameters,
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
    let email: String = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(internal)?
        .ok_or_else(|| crate::error::not_found(ErrorCode::UserNotFound, "account not found"))?;

    let token = crate::services::email_verification::issue_token(&state.db, claims.sub)
        .await
        .map_err(internal)?;
    crate::services::email_verification::send_verification_email(&email, &token);

    Ok(Json(serde_json::json!({ "sent": true })))
}

/// Stores the requesting address on an OTP challenge for audit. Best effort:
/// failing to record it must not fail the signup or login.
async fn record_challenge_ip(state: &AppState, challenge_id: uuid::Uuid, ip: Option<std::net::IpAddr>) {
    let Some(ip) = ip else { return };
    if let Err(err) = sqlx::query("UPDATE otp_challenges SET client_ip = $2::inet WHERE id = $1")
        .bind(challenge_id)
        .bind(ip.to_string())
        .execute(&state.db)
        .await
    {
        tracing::warn!(error = %err, "failed to record OTP challenge client ip");
    }
}
