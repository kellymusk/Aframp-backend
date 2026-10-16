use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub merchant_id: Option<Uuid>,
    #[serde(default)]
    pub is_admin: bool,
    pub exp: usize,
    pub iat: usize,
    /// When the session was first issued (login/verify-otp). Carried
    /// unchanged through refreshes so a session can't be extended past
    /// [`MAX_SESSION_DAYS`]. `0` on tokens issued before this claim existed,
    /// which then fall back to `iat`.
    #[serde(default)]
    pub orig_iat: usize,
}

pub const TOKEN_TTL_HOURS: i64 = 24;

/// Upper bound on a refreshed session, measured from the original login.
pub const MAX_SESSION_DAYS: i64 = 7;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RefreshError {
    #[error("session can no longer be refreshed; log in again")]
    WindowExpired,
    #[error("token issue time is invalid")]
    InvalidIssuedAt,
    #[error("failed to sign token")]
    Signing,
}

pub fn sign(
    secret: &str,
    user_id: Uuid,
    merchant_id: Option<Uuid>,
    is_admin: bool,
) -> Result<String, jsonwebtoken::errors::Error> {
    let now = Utc::now();
    let iat = now.timestamp() as usize;
    let claims = Claims {
        sub: user_id,
        merchant_id,
        is_admin,
        iat,
        exp: (now + Duration::hours(TOKEN_TTL_HOURS)).timestamp() as usize,
        orig_iat: iat,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

pub fn verify(secret: &str, token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|d| d.claims)
}

/// Issue a fresh token for an already-verified, unexpired session. The new
/// token keeps the original login time, expires [`TOKEN_TTL_HOURS`] from now
/// but never later than [`MAX_SESSION_DAYS`] after that login, and is refused
/// once that window has passed, so refreshes can't chain forever.
pub fn refresh(secret: &str, claims: &Claims) -> Result<String, RefreshError> {
    refresh_at(secret, claims, Utc::now().timestamp())
}

fn refresh_at(secret: &str, claims: &Claims, now: i64) -> Result<String, RefreshError> {
    let iat = claims.iat as i64;
    let orig_iat = if claims.orig_iat == 0 { iat } else { claims.orig_iat as i64 };
    // A token issued in the future, or before its own session began, wasn't
    // produced by this server's sign/refresh.
    if iat > now || iat < orig_iat {
        return Err(RefreshError::InvalidIssuedAt);
    }
    let window_end = orig_iat + Duration::days(MAX_SESSION_DAYS).num_seconds();
    if now >= window_end {
        return Err(RefreshError::WindowExpired);
    }

    let refreshed = Claims {
        sub: claims.sub,
        merchant_id: claims.merchant_id,
        is_admin: claims.is_admin,
        iat: now as usize,
        exp: (now + Duration::hours(TOKEN_TTL_HOURS).num_seconds()).min(window_end) as usize,
        orig_iat: orig_iat as usize,
    };
    encode(&Header::default(), &refreshed, &EncodingKey::from_secret(secret.as_bytes()))
        .map_err(|_| RefreshError::Signing)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-secret";
    const DAY: i64 = 86_400;

    fn claims(iat: i64, orig_iat: i64) -> Claims {
        Claims {
            sub: Uuid::new_v4(),
            merchant_id: None,
            is_admin: false,
            iat: iat as usize,
            exp: (iat + DAY) as usize,
            orig_iat: orig_iat as usize,
        }
    }

    fn decode_unchecked(token: &str) -> Claims {
        let mut validation = Validation::default();
        validation.validate_exp = false;
        decode::<Claims>(token, &DecodingKey::from_secret(SECRET.as_bytes()), &validation)
            .unwrap()
            .claims
    }

    #[test]
    fn refresh_keeps_the_original_login_time_and_extends_expiry() {
        let now = 1_800_000_000;
        let login = now - 2 * DAY;
        let token = refresh_at(SECRET, &claims(now - 3_600, login), now).unwrap();
        let refreshed = decode_unchecked(&token);
        assert_eq!(refreshed.orig_iat as i64, login);
        assert_eq!(refreshed.iat as i64, now);
        assert_eq!(refreshed.exp as i64, now + DAY);
    }

    #[test]
    fn refreshed_expiry_never_passes_the_session_window() {
        let now = 1_800_000_000;
        let login = now - 6 * DAY - 3_600; // one hour of window left
        let refreshed = decode_unchecked(&refresh_at(SECRET, &claims(now - 60, login), now).unwrap());
        assert_eq!(refreshed.exp as i64, login + MAX_SESSION_DAYS * DAY);
    }

    #[test]
    fn refresh_is_refused_once_the_window_has_passed() {
        let now = 1_800_000_000;
        let login = now - MAX_SESSION_DAYS * DAY;
        assert_eq!(refresh_at(SECRET, &claims(now - 60, login), now), Err(RefreshError::WindowExpired));
    }

    #[test]
    fn refresh_rejects_inconsistent_issue_times() {
        let now = 1_800_000_000;
        assert_eq!(refresh_at(SECRET, &claims(now + 60, now - DAY), now), Err(RefreshError::InvalidIssuedAt));
        assert_eq!(refresh_at(SECRET, &claims(now - DAY, now - 60), now), Err(RefreshError::InvalidIssuedAt));
    }

    #[test]
    fn legacy_tokens_without_orig_iat_use_iat_as_the_session_start() {
        let now = 1_800_000_000;
        let refreshed = decode_unchecked(&refresh_at(SECRET, &claims(now - 60, 0), now).unwrap());
        assert_eq!(refreshed.orig_iat as i64, now - 60);
    }
}
