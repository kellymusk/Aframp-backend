//! Session cookie handling.
//!
//! The JWT is also returned in the login/signup JSON body for API clients, but
//! browsers should use this cookie instead: `HttpOnly` keeps the token out of
//! `document.cookie` and `localStorage`, so an XSS on the merchant dashboard
//! can no longer read the session and replay it elsewhere.

use axum::http::header::{HeaderMap, HeaderValue, InvalidHeaderValue, COOKIE};

use crate::auth::jwt::TOKEN_TTL_HOURS;

pub const SESSION_COOKIE: &str = "aframp_session";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    Lax,
    None,
}

impl SameSite {
    fn as_str(self) -> &'static str {
        match self {
            SameSite::Lax => "Lax",
            SameSite::None => "None",
        }
    }
}

/// How session cookies are stamped. `Lax` + same-origin deployment is the
/// supported setup; `None` exists for a frontend served from a different
/// origin and requires `Secure`, which browsers enforce themselves.
#[derive(Debug, Clone, Copy)]
pub struct CookieConfig {
    pub secure: bool,
    pub same_site: SameSite,
}

impl CookieConfig {
    /// `Set-Cookie` value installing a session for the lifetime of the token.
    pub fn session(&self, token: &str) -> Result<HeaderValue, InvalidHeaderValue> {
        self.build(token, TOKEN_TTL_HOURS * 60 * 60)
    }

    /// `Set-Cookie` value expiring the session immediately.
    pub fn clear(&self) -> Result<HeaderValue, InvalidHeaderValue> {
        self.build("", 0)
    }

    fn build(&self, token: &str, max_age_secs: i64) -> Result<HeaderValue, InvalidHeaderValue> {
        let mut value = format!(
            "{SESSION_COOKIE}={token}; HttpOnly; Path=/; SameSite={}; Max-Age={max_age_secs}",
            self.same_site.as_str()
        );
        if self.secure {
            value.push_str("; Secure");
        }
        HeaderValue::from_str(&value)
    }
}

/// Pull the session token out of the request's `Cookie` header, if present.
pub fn from_headers(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| name.trim() == SESSION_COOKIE)
        .map(|(_, token)| token.trim())
        .filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(cookie: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, HeaderValue::from_str(cookie).unwrap());
        headers
    }

    #[test]
    fn session_cookie_is_http_only_and_scoped_to_the_whole_site() {
        let config = CookieConfig { secure: true, same_site: SameSite::Lax };
        let value = config.session("jwt.goes.here").unwrap();
        let value = value.to_str().unwrap();
        assert!(value.starts_with("aframp_session=jwt.goes.here;"));
        assert!(value.contains("; HttpOnly"));
        assert!(value.contains("; Path=/"));
        assert!(value.contains("; SameSite=Lax"));
        assert!(value.contains("; Secure"));
        assert!(value.contains("; Max-Age=86400"));
    }

    #[test]
    fn secure_is_omitted_when_disabled_for_plain_http_dev() {
        let config = CookieConfig { secure: false, same_site: SameSite::Lax };
        let value = config.session("t").unwrap();
        assert!(!value.to_str().unwrap().contains("Secure"));
    }

    #[test]
    fn clearing_expires_the_cookie_immediately() {
        let config = CookieConfig { secure: true, same_site: SameSite::Lax };
        let value = config.clear().unwrap();
        assert!(value.to_str().unwrap().contains("; Max-Age=0"));
    }

    #[test]
    fn reads_the_session_among_other_cookies() {
        let headers = headers("theme=dark; aframp_session=abc.def; lang=en");
        assert_eq!(from_headers(&headers), Some("abc.def"));
    }

    #[test]
    fn ignores_cookies_with_a_similar_name() {
        assert_eq!(from_headers(&headers("aframp_session_old=abc")), None);
        assert_eq!(from_headers(&headers("not_aframp_session=abc")), None);
    }

    #[test]
    fn treats_a_cleared_cookie_as_absent() {
        assert_eq!(from_headers(&headers("aframp_session=")), None);
    }

    #[test]
    fn missing_cookie_header_is_none() {
        assert_eq!(from_headers(&HeaderMap::new()), None);
    }
}
