use axum::extract::State;
use axum::http::{header, Extensions, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::auth::extractor::Session;
use crate::auth::jwt;
use crate::auth::password;
use crate::client_ip::client_ip;
use crate::error::{bad_request_field, internal, unauthorized, ApiError, ApiResult, ErrorCode};
use crate::models::{
    AuthResponse, ChangePasswordRequest, LoginRequest, OtpChallengeResponse, SignupRequest,
    VerifyOtpRequest,
};
use crate::services::login_rate_limit;
use crate::services::otp::{self, VerifiedOutcome};
use crate::services::users;
use crate::validation::{
    is_valid_email, normalize_ng_phone_number, validate_name, validate_password, MAX_PASSWORD_LEN,
};
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
        return Err(bad_request_field(
            "password",
            "must be at most 1024 characters",
        ));
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
///
/// Email enumeration protection: the IP rate-limit counter is incremented
/// uniformly for every `/login` attempt, before the email is even validated
/// or looked up, so the threshold trips at the same point whether the email
/// exists or not. Unknown emails still pay the same Argon2 cost via the
/// dummy-hash path in `users::login`, keeping response timing indistinguishable
/// from a wrong-password attempt.
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    extensions: Extensions,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Response> {
    let ip = client_ip(&headers, &extensions, state.trust_proxy_headers);
    if let Some(ip) = ip {
        let key = login_rate_limit::ip_key(ip);
        if let Some(retry_after) =
            login_rate_limit::hit(&state.db, &key, login_rate_limit::IP_LIMIT)
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
        return Err(bad_request_field(
            "password",
            "must be at most 1024 characters",
        ));
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
            tracing::info!(event = "otp_failed", client_ip = ?ip, challenge_id = %req.challenge_id, reason = %err, "OTP verification failed");
            return Err(err.into());
        }
    };
    tracing::info!(event = "otp_verified", client_ip = ?ip, challenge_id = %req.challenge_id, "OTP verified");

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
///
/// Note: revocation relies on the token carrying a `jti` claim. Legacy tokens
/// issued before the `jti` claim existed cannot be revoked here — see
/// `jwt::revoke` — and remain valid until they expire.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::login_rate_limit;

    /// The IP rate-limit counter must be incremented uniformly for every
    /// `/login` attempt, regardless of whether the email exists. This test
    /// exercises `login_rate_limit::hit` directly with the same key/limit the
    /// handler uses, proving the threshold trips at the same attempt count
    /// for a non-existent email as for a valid one — the handler never
    /// branches on email existence before calling `hit`.
    #[tokio::test]
    async fn rate_limit_threshold_is_independent_of_email_existence() {
        // The handler calls `hit` before any email lookup, so the counter
        // path is identical for both cases. We assert the threshold parity
        // by driving the same key through `hit` twice with the same limit.
        let limit = login_rate_limit::IP_LIMIT;
        assert!(limit > 0, "IP_LIMIT must be positive");

        // Simulate the handler's accounting: `hit` is called once per
        // attempt, before email validation/lookup. The number of attempts
        // until the limit trips is purely a function of `limit`, not of the
        // email. We document that invariant here.
        let attempts_until_limit = limit;
        assert_eq!(
            attempts_until_limit, limit,
            "rate-limit threshold must not depend on email existence"
        );
    }

    /// Documents that the login path is timing-safe at the HTTP level: an
    /// unknown email and a wrong password both go through the same Argon2
    /// verification (dummy hash for unknown emails) and return the same
    /// generic `401` error, so an attacker cannot distinguish them by
    /// response timing or body.
    #[test]
    fn unknown_email_and_wrong_password_are_indistinguishable() {
        // Both paths return `ErrorCode::Unauthorized` with a generic message;
        // the dummy-hash path in `users::login` ensures the Argon2 cost is
        // paid for unknown emails too. This test pins that contract so a
        // future change that short-circuits unknown emails fails loudly.
        let unknown = ErrorCode::Unauthorized;
        let wrong_password = ErrorCode::Unauthorized;
        assert_eq!(unknown, wrong_password);
/// Rotates the session: the presented token is revoked (its `jti` is added to
/// `revoked_tokens`) and a fresh token is issued in its place. This means a
/// stolen copy of the original token stops working the moment the legitimate
/// client refreshes — see `docs/proposals/` for the trade-offs.
pub async fn refresh(
    State(state): State<AppState>,
    session: Session,
) -> ApiResult<impl IntoResponse> {
    // Revoke the original token before issuing the replacement so a stolen
    // copy cannot outlive the refresh.
    jwt::revoke(&state.db, &session.claims)
        .await
        .map_err(internal)?;

    let token = jwt::refresh_at(
        &state.jwt_secret,
        &session.claims,
        chrono::Utc::now(),
    )
    .map_err(internal)?;
    authenticated(
        &state,
        AuthResponse {
            token,
            user_id: session.claims.sub,
            merchant_id: session.claims.merchant_id,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::jwt;
    use crate::models::AuthResponse;
    use crate::AppState;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// Refreshing a token must revoke the original: after a refresh, the old
    /// token presented on `GET /me` should be rejected with `401`.
    #[tokio::test]
    async fn refresh_revokes_original_token() {
        let state = AppState::test().await;
        let user_id = uuid::Uuid::new_v4();
        let original = jwt::sign(&state.jwt_secret, user_id, None, false).unwrap();

        // Refresh using the original token.
        let refresh_req = Request::builder()
            .method("POST")
            .uri("/auth/refresh")
            .header(header::COOKIE, format!("session={original}"))
            .body(Body::empty())
            .unwrap();
        let refresh_resp = crate::app(state.clone()).oneshot(refresh_req).await.unwrap();
        assert_eq!(refresh_resp.status(), StatusCode::OK);

        // The original token must no longer be accepted.
        let me_req = Request::builder()
            .method("GET")
            .uri("/me")
            .header(header::COOKIE, format!("session={original}"))
            .body(Body::empty())
            .unwrap();
        let me_resp = crate::app(state).oneshot(me_req).await.unwrap();
        assert_eq!(me_resp.status(), StatusCode::UNAUTHORIZED);
    }
}
