//! Tests for middleware scope: `GET /health` and `/admin` must work without
//! a `Content-Type` header.
//!
//! **Issue #1275** — `require_json_content_type` is applied as a blanket
//! router-level layer in `src/lib.rs`.  Although the middleware already
//! short-circuits on non-POST/PUT methods (so `GET /health` and `GET /admin`
//! are *functionally* unaffected), the intent is clearer if the middleware is
//! scoped only to the JSON API routes.
//!
//! These tests document and lock in the *current correct behaviour*: `GET
//! /health` must succeed and must not require any
//! `Content-Type` header.  They will continue to pass before and after the
//! refactor lands (issue #1275), making them a regression guard.
//!
//! ## Running
//!
//! ```bash
//! # Without a database (all tests here skip DB work):
//! cargo test --test middleware_scope
//!
//! # With a database (for the #[ignore] tests below):
//! TEST_DATABASE_URL=postgres://... cargo test --test middleware_scope -- --ignored
//! ```

mod common;

use aframp::router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Send a bare GET request with no headers except the mandatory Host.
async fn get_no_headers(app: axum::Router, uri: &str) -> StatusCode {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}

/// Send a GET with an explicit (wrong) Content-Type to confirm GET routes
/// ignore it.
async fn get_with_content_type(app: axum::Router, uri: &str, ct: &str) -> StatusCode {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .header("content-type", ct)
        .body(Body::empty())
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}

// ── /health tests ─────────────────────────────────────────────────────────────

/// `GET /health` must succeed whether or not a `Content-Type` header is present.
#[tokio::test]
async fn health_succeeds_without_content_type_header() {
    let state = common::state().await;
    let app = router(state);

    let status = get_no_headers(app, "/health").await;
    assert!(
        status.is_success(),
        "GET /health must return 2xx even without a Content-Type header"
    );
}

/// `GET /health` must succeed even when the client sends a wrong
/// content-type (e.g. `text/plain`).  The middleware must not reject GETs.
#[tokio::test]
async fn health_succeeds_with_wrong_content_type() {
    let state = common::state().await;
    let app = router(state);

    let status = get_with_content_type(app, "/health", "text/plain").await;
    assert!(
        status.is_success(),
        "GET /health must return 2xx even with a non-JSON Content-Type"
    );
}

/// `GET /health` must not return `415 Unsupported Media Type` — that code is
/// reserved for POST/PUT requests with a body and a wrong content-type.
#[tokio::test]
async fn health_is_never_415() {
    let state = common::state().await;
    let app = router(state);

    let status = get_no_headers(app, "/health").await;
    assert_ne!(
        status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "GET /health must never return 415 — the middleware must not apply to GET routes"
    );
}

// ── /admin tests ──────────────────────────────────────────────────────────────

/// `GET /admin` must not return `415`.  It serves an HTML page and should be
/// exempt from the JSON content-type middleware regardless of its scope.
#[tokio::test]
async fn admin_dashboard_is_not_415() {
    let state = common::state().await;
    let app = router(state);

    let status = get_no_headers(app, "/admin").await;
    assert_ne!(
        status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "GET /admin must never return 415"
    );
}

/// `GET /admin` with a non-JSON content-type must not return `415`.
#[tokio::test]
async fn admin_with_html_content_type_is_not_415() {
    let state = common::state().await;
    let app = router(state);

    let status = get_with_content_type(app, "/admin", "text/html").await;
    assert_ne!(
        status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "GET /admin with text/html must not return 415"
    );
}

// ── middleware still applies to POST with a body ──────────────────────────────

/// The middleware must still reject a `POST` to `/signup` with a wrong
/// content-type and a non-empty body.  This is the positive case: the
/// refactor must NOT remove the middleware from JSON API routes.
#[tokio::test]
async fn post_with_wrong_content_type_still_returns_415() {
    let state = common::state().await;
    let app = router(state);

    let body = b"not json at all";
    let req = Request::builder()
        .method("POST")
        .uri("/signup")
        .header("content-type", "text/plain")
        .header("content-length", body.len().to_string())
        .body(Body::from(body.as_ref()))
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "POST /signup with a body and wrong content-type must still return 415"
    );
}

// ── route → middleware scope mapping ─────────────────────────────────────────
//
// Every route registered in `src/lib.rs` must appear in exactly one of the
// tables below.  When a new route is added, add it here too — a route that
// lands in the wrong scope (e.g. a protected route exposed publicly) will then
// fail one of the tests in this section.
//
// Scope legend:
//   PUBLIC   — no auth middleware; must NOT answer 401 without a token.
//   MERCHANT — merchant auth required; must answer 401 without a token.
//   ADMIN    — admin auth required; must answer 401 or 403 without an admin
//              token.
//
// PUBLIC routes:
//   GET  /health
//   GET  /health/detailed
//   GET  /admin
//   POST /signup
//   POST /login
//   POST /refresh
//   GET  /assets
//   GET  /assets/{id}
//   GET  /assets/{id}/history
//   GET  /orderbook/{base}/{quote}
//   GET  /trades/{base}/{quote}
//   GET  /stats
//   GET  /fee-estimate
//   GET  /transactions/{hash}
//   GET  /wallets/{address}/balances
//   GET  /wallets/{address}/transactions
//   GET  /payment-requests/{id}/status
//   GET  /payment-requests/{id}/qr
//   POST /payment-requests/{id}/expire
//   POST /payment-requests/{id}/cancel
//   GET  /webhooks/stellar
//   POST /webhooks/stellar
//
// MERCHANT routes (merchant auth required):
//   GET  /me
//   GET  /me/transactions
//   GET  /me/payment-requests
//   POST /payment-requests
//   GET  /payment-requests/{id}
//   POST /payments
//   GET  /payments/{id}
//   POST /withdrawals
//   GET  /withdrawals/{id}
//   GET  /api-keys
//   POST /api-keys
//   DELETE /api-keys/{id}
//   GET  /settings
//   PUT  /settings
//
// ADMIN routes (admin auth required):
//   GET  /admin/dashboard
//   GET  /admin/events
//   GET  /admin/merchants
//   GET  /admin/merchants/{id}
//   POST /admin/merchants/{id}/suspend
//   POST /admin/merchants/{id}/unsuspend
//   GET  /admin/users
//   GET  /admin/users/{id}
//   POST /admin/users/{id}/unlock
//   GET  /admin/transactions
//   GET  /admin/payment-requests
//   GET  /admin/withdrawals
//   GET  /admin/settings
//   PUT  /admin/settings
//   GET  /admin/api-keys

/// Routes that must be reachable without any auth token.  A 401 here means a
/// public route was accidentally placed behind an auth layer.
const PUBLIC_ROUTES: &[&str] = &[
    "/health",
    "/health/detailed",
    "/admin",
    "/assets",
    "/assets/1",
    "/assets/1/history",
    "/orderbook/XLM/USDC",
    "/trades/XLM/USDC",
    "/stats",
    "/fee-estimate",
    "/transactions/abc123",
    "/wallets/GABC/balances",
    "/wallets/GABC/transactions",
    "/payment-requests/1/status",
    "/payment-requests/1/qr",
    "/webhooks/stellar",
];

/// Routes that require merchant auth.  Without a token they must answer 401.
const MERCHANT_ROUTES: &[&str] = &[
    "/me",
    "/me/transactions",
    "/me/payment-requests",
    "/payment-requests/1",
    "/payments/1",
    "/withdrawals/1",
    "/api-keys",
    "/settings",
];

/// Routes that require admin auth.  Without an admin token they must answer
/// 401 or 403.
const ADMIN_ROUTES: &[&str] = &[
    "/admin/dashboard",
    "/admin/events",
    "/admin/merchants",
    "/admin/merchants/1",
    "/admin/users",
    "/admin/users/1",
    "/admin/transactions",
    "/admin/payment-requests",
    "/admin/withdrawals",
    "/admin/settings",
    "/admin/api-keys",
];

/// `(method, uri)` pairs for write endpoints that require merchant auth.
const MERCHANT_WRITE_ROUTES: &[(&str, &str)] = &[
    ("POST", "/payment-requests"),
    ("POST", "/payments"),
    ("POST", "/withdrawals"),
    ("POST", "/api-keys"),
    ("DELETE", "/api-keys/1"),
    ("PUT", "/settings"),
];

/// `(method, uri)` pairs for admin write endpoints.
const ADMIN_WRITE_ROUTES: &[(&str, &str)] = &[
    ("POST", "/admin/merchants/1/suspend"),
    ("POST", "/admin/merchants/1/unsuspend"),
    ("POST", "/admin/users/1/unlock"),
    ("PUT", "/admin/settings"),
];

/// Send a request with no auth headers and return the status code.
async fn send_no_auth(app: axum::Router, method: &str, uri: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}

// ── middleware scope: public routes ──────────────────────────────────────────

/// Every public route must be reachable without a token (never 401).
#[tokio::test]
async fn public_routes_do_not_require_auth() {
    for uri in PUBLIC_ROUTES {
        let app = router(common::state().await);
        let status = send_no_auth(app, "GET", uri).await;
        assert_ne!(
            status,
            StatusCode::UNAUTHORIZED,
            "public route {uri} must not require auth, got {status}"
        );
    }
}

// ── middleware scope: merchant routes ────────────────────────────────────────

/// Every merchant-auth route must reject an unauthenticated request with 401.
#[tokio::test]
async fn merchant_routes_require_auth() {
    for uri in MERCHANT_ROUTES {
        let app = router(common::state().await);
        let status = send_no_auth(app, "GET", uri).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "merchant route {uri} must require auth, got {status}"
        );
    }

    for (method, uri) in MERCHANT_WRITE_ROUTES {
        let app = router(common::state().await);
        let status = send_no_auth(app, method, uri).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "merchant route {method} {uri} must require auth, got {status}"
        );
    }
}

// ── middleware scope: admin routes ───────────────────────────────────────────

/// Every admin route must reject an unauthenticated request with 401 or 403.
#[tokio::test]
async fn admin_routes_require_admin_auth() {
    for uri in ADMIN_ROUTES {
        let app = router(common::state().await);
        let status = send_no_auth(app, "GET", uri).await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
            "admin route {uri} must require admin auth, got {status}"
        );
    }

    for (method, uri) in ADMIN_WRITE_ROUTES {
        let app = router(common::state().await);
        let status = send_no_auth(app, method, uri).await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
            "admin route {method} {uri} must require admin auth, got {status}"
        );
    }
}

// ── documentation / design tests (no DB) ─────────────────────────────────────

/// `/health/detailed` reports each dependency; the test Horizon is
/// unreachable, so it answers 503 with the database still `ok`.
#[tokio::test]
async fn detailed_health_reports_each_dependency() {
    let app = router(common::state().await);
    let (status, body) = common::send(app, "GET", "/health/detailed", None, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["status"], "degraded");
    assert_eq!(body["checks"]["database"], "ok");
    assert_eq!(body["checks"]["horizon"], "fail");
}

#[tokio::test]
async fn detailed_health_is_ok_when_dependencies_answer() {
    let horizon = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .mount(&horizon)
        .await;
    let mut state = common::state().await;
    state.stellar_horizon_url = std::sync::Arc::new(horizon.uri());

    let (status, body) = common::send(router(state), "GET", "/health/detailed", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["checks"]["horizon"], "ok");
}
