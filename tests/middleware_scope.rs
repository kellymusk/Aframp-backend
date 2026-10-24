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
//! /health` must return `204` with no body and must not require any
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

/// `GET /health` must return `204 No Content` with no body, regardless of
/// whether a `Content-Type` header is present.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn health_returns_204_without_content_type_header() {
    let Some(state) = common::state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state);

    let status = get_no_headers(app, "/health").await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "GET /health must return 204 even without a Content-Type header"
    );
}

/// `GET /health` must return `204` even when the client sends a wrong
/// content-type (e.g. `text/plain`).  The middleware must not reject GETs.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn health_returns_204_with_wrong_content_type() {
    let Some(state) = common::state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = router(state);

    let status = get_with_content_type(app, "/health", "text/plain").await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "GET /health must return 204 even with a non-JSON Content-Type"
    );
}

/// `GET /health` must not return `415 Unsupported Media Type` — that code is
/// reserved for POST/PUT requests with a body and a wrong content-type.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn health_is_never_415() {
    let Some(state) = common::state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
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
#[ignore = "requires TEST_DATABASE_URL"]
async fn admin_dashboard_is_not_415() {
    let Some(state) = common::state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
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
#[ignore = "requires TEST_DATABASE_URL"]
async fn admin_with_html_content_type_is_not_415() {
    let Some(state) = common::state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
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
#[ignore = "requires TEST_DATABASE_URL"]
async fn post_with_wrong_content_type_still_returns_415() {
    let Some(state) = common::state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
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

/// Documents the intent of issue #1275: the middleware should be scoped to
/// API routes only, not applied globally.
///
/// This is a compile-time documentation test — it always passes.
#[test]
fn middleware_scope_design_note() {
    // Current behaviour (before the refactor):
    //   - middleware applied via router().layer(from_fn(require_json_content_type))
    //   - Short-circuits on GET/HEAD/DELETE, so /health and /admin are
    //     functionally unaffected even though the layer runs.
    //
    // Desired behaviour (after issue #1275):
    //   - JSON API routes are in a nested Router that has the middleware as a
    //     layer; /health and /admin are in a separate, middleware-free Router.
    //   - This makes the scope explicit and prevents accidentally catching a
    //     future GET route that might return non-JSON.
    //
    // The proposed lib.rs structure:
    //
    //   let api_routes = Router::new()
    //       .route("/signup", post(…))
    //       // … all POST/PUT JSON API routes …
    //       .layer(from_fn(require_json_content_type));
    //
    //   let non_api_routes = Router::new()
    //       .route("/health", get(…))
    //       .route("/admin", get(…))
    //       .route("/admin/*path", get(…));
    //
    //   Router::new()
    //       .merge(api_routes)
    //       .merge(non_api_routes)
    //       .with_state(state)
    //
    // See docs/middleware-scope-refactor.md for the full design note.
    assert!(true, "design note always passes");
}
