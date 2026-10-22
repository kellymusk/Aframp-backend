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
