/// #1043 — Tests for CORS preflight rejection of unlisted origins.
///
/// The README documents: "an unlisted origin fails preflight" and CORS_ALLOWED_ORIGINS
/// is an explicit allowlist, never mirrored back.  These tests verify that the
/// CorsLayer configured in main.rs enforces that contract.
///
/// Because the CorsLayer is applied outside `router()` (in `main.rs`), the
/// tests build their own layered app here using the same tower_http::cors
/// setup — ensuring the middleware logic is exercised, not just the raw router.
mod common;

use axum::body::Body;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::Router;
use tower::ServiceExt;
use tower_http::cors::CorsLayer;

use common::{ensure_merchant, state};

// The origin that is in the allowlist for these tests.
const ALLOWED_ORIGIN: &str = "http://localhost:3001";
// An origin that is NOT in the allowlist.
const BLOCKED_ORIGIN: &str = "http://evil.example.com";

/// Builds an app with the same CorsLayer configuration as main.rs, wrapping
/// the standard `aframp::router`.
fn cors_app(state: aframp::AppState, allowed_origins: &[&str]) -> Router {
    let origins: Vec<HeaderValue> = allowed_origins
        .iter()
        .map(|o| o.parse().expect("valid origin"))
        .collect();

    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    aframp::router(state).layer(cors)
}

/// Send an OPTIONS preflight request with the given `Origin` and
/// `Access-Control-Request-Method` headers.
async fn preflight(
    app: Router,
    uri: &str,
    origin: &str,
    method: &str,
) -> (StatusCode, axum::http::HeaderMap) {
    let req = Request::builder()
        .method(Method::OPTIONS)
        .uri(uri)
        .header(header::ORIGIN, origin)
        .header("access-control-request-method", method)
        .header("access-control-request-headers", "content-type")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    (status, headers)
}

/// Send a GET request with an `Origin` header (simulates a cross-origin fetch).
async fn cross_origin_get(
    app: Router,
    uri: &str,
    origin: &str,
    token: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap) {
    let mut builder = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::ORIGIN, origin)
        .header("content-type", "application/json");
    if let Some(t) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = builder.body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    (status, headers)
}

// ─────────────────────────────────────────────────────────────────────────────
// #1043 test 1: OPTIONS preflight from an unlisted origin gets no ACAO header
// ─────────────────────────────────────────────────────────────────────────────

/// An OPTIONS request from an origin not in the allowlist must NOT receive an
/// `Access-Control-Allow-Origin` response header.  The browser will block the
/// actual request when that header is absent, which is the correct behaviour.
#[tokio::test]
async fn preflight_from_unlisted_origin_has_no_acao_header() {
    let Some(state) = state().await else {
        return;
    };
    let app = cors_app(state, &[ALLOWED_ORIGIN]);

    let (_status, headers) =
        preflight(app, "/wallet/create", BLOCKED_ORIGIN, "POST").await;

    assert!(
        headers.get("access-control-allow-origin").is_none(),
        "unlisted origin must not receive Access-Control-Allow-Origin; got: {:?}",
        headers.get("access-control-allow-origin")
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// #1043 test 2: OPTIONS preflight from a listed origin gets correct CORS headers
// ─────────────────────────────────────────────────────────────────────────────

/// An OPTIONS request from the allowed origin must receive an
/// `Access-Control-Allow-Origin` header that reflects exactly that origin
/// (never a wildcard), plus `Access-Control-Allow-Credentials: true` (required
/// for cookie-carrying requests).
#[tokio::test]
async fn preflight_from_listed_origin_returns_correct_cors_headers() {
    let Some(state) = state().await else {
        return;
    };
    let app = cors_app(state, &[ALLOWED_ORIGIN]);

    let (status, headers) =
        preflight(app, "/wallet/create", ALLOWED_ORIGIN, "POST").await;

    // tower_http::cors returns 200 for a successful preflight.
    assert!(
        status.is_success(),
        "preflight from listed origin should succeed, got {status}"
    );

    let acao = headers
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(
        acao, ALLOWED_ORIGIN,
        "ACAO header must echo the exact allowed origin, not a wildcard"
    );

    let acac = headers
        .get("access-control-allow-credentials")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(
        acac, "true",
        "ACAC header must be 'true' so browsers send cookies on credentialed requests"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// #1043 test 3: GET /me from listed origin with valid token succeeds
// ─────────────────────────────────────────────────────────────────────────────

/// A credentialed GET from the allowed origin must succeed (200) and receive
/// a permissive `Access-Control-Allow-Origin` header so the browser can read
/// the response body.
#[tokio::test]
async fn get_me_from_listed_origin_with_token_succeeds() {
    let Some(state) = state().await else {
        return;
    };
    // Build a bare (no-CORS) router just for the sign-up flow — the token is
    // all we need; no cross-origin headers are required here.
    let bare_app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&bare_app, "cors_me_ok").await;

    // Now exercise the CORS-layered app.
    let app = cors_app(state, &[ALLOWED_ORIGIN]);
    let (status, headers) =
        cross_origin_get(app, "/me", ALLOWED_ORIGIN, Some(&token)).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "GET /me from listed origin with valid token must return 200"
    );

    let acao = headers
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(
        acao, ALLOWED_ORIGIN,
        "response must carry ACAO header so the browser can read the body"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// #1043 test 4: GET /me from unlisted origin gets no ACAO header
// ─────────────────────────────────────────────────────────────────────────────

/// A cross-origin GET from a blocked origin must not receive an
/// `Access-Control-Allow-Origin` header — the browser will refuse to expose
/// the response body, which is the correct security posture even if the
/// underlying handler would have returned 200.
#[tokio::test]
async fn get_me_from_unlisted_origin_has_no_acao_header() {
    let Some(state) = state().await else {
        return;
    };
    let bare_app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&bare_app, "cors_me_blocked").await;

    let app = cors_app(state, &[ALLOWED_ORIGIN]);
    let (_status, headers) =
        cross_origin_get(app, "/me", BLOCKED_ORIGIN, Some(&token)).await;

    assert!(
        headers.get("access-control-allow-origin").is_none(),
        "unlisted origin must not receive Access-Control-Allow-Origin; got: {:?}",
        headers.get("access-control-allow-origin")
    );
}
