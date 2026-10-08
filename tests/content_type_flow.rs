/// Integration tests for #1044: content-type enforcement middleware on real routes.
///
/// The `require_json_content_type` middleware has unit tests in `src/middleware.rs`
/// that use a synthetic router. These tests verify that the same 415 behaviour
/// fires on the real application routes (`/withdraw`, `/payment-requests`, `/login`)
/// — i.e. the middleware is actually wired into the production router.
mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;

use common::state;

async fn app() -> Option<axum::Router> {
    state().await.map(aframp::router)
}

/// Sends a raw request with explicit control over the `Content-Type` header
/// and body so we can test the middleware without the helper's forced
/// `application/json`.
async fn raw_send(
    app: axum::Router,
    method: &str,
    uri: &str,
    content_type: Option<&str>,
    body: &str,
) -> StatusCode {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(ct) = content_type {
        builder = builder.header(header::CONTENT_TYPE, ct);
    }
    if !body.is_empty() {
        builder = builder.header(header::CONTENT_LENGTH, body.len().to_string());
    }
    let request = builder
        .body(Body::from(body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    response.status()
}

// ── /login ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn login_with_no_content_type_but_a_body_returns_415() {
    let Some(app) = app().await else {
        return;
    };
    // No Content-Type, non-empty body — the middleware must reject it before the
    // handler ever runs.
    let status = raw_send(
        app,
        "POST",
        "/login",
        None,
        r#"{"email":"x@x.com","password":"password123"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn login_with_text_plain_body_returns_415() {
    let Some(app) = app().await else {
        return;
    };
    let status = raw_send(
        app,
        "POST",
        "/login",
        Some("text/plain"),
        r#"{"email":"x@x.com","password":"password123"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn login_415_error_body_has_expected_shape() {
    let Some(app) = app().await else {
        return;
    };
    let body_str = r#"{"email":"x@x.com","password":"password123"}"#;
    let request = Request::builder()
        .method("POST")
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/xml")
        .header(header::CONTENT_LENGTH, body_str.len().to_string())
        .body(Body::from(body_str))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    // The error body must follow the {error, code} shape from error.rs.
    assert!(
        json.get("error").and_then(|v| v.as_str()).is_some(),
        "expected an 'error' string field in the response: {json}"
    );
    assert!(
        json.get("code").and_then(|v| v.as_str()).is_some(),
        "expected a 'code' string field in the response: {json}"
    );
    assert_eq!(
        json["code"], "INVALID_PARAMETERS",
        "expected INVALID_PARAMETERS code: {json}"
    );
}

// ── /payment-requests ─────────────────────────────────────────────────────────

#[tokio::test]
async fn payment_requests_post_with_application_xml_returns_415() {
    let Some(app) = app().await else {
        return;
    };
    let body_str = r#"{"amount_stroops":25000000}"#;
    let status = raw_send(
        app,
        "POST",
        "/payment-requests",
        Some("application/xml"),
        body_str,
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn payment_requests_post_with_text_plain_returns_415() {
    let Some(app) = app().await else {
        return;
    };
    let status = raw_send(
        app,
        "POST",
        "/payment-requests",
        Some("text/plain"),
        r#"{"amount_stroops":25000000}"#,
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn payment_requests_post_with_no_content_type_but_body_returns_415() {
    let Some(app) = app().await else {
        return;
    };
    let status = raw_send(
        app,
        "POST",
        "/payment-requests",
        None,
        r#"{"amount_stroops":25000000}"#,
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

// ── /withdraw ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn withdraw_post_with_text_plain_returns_415() {
    let Some(app) = app().await else {
        return;
    };
    let body_str = r#"{"amount_stroops":500000000,"bank_code":"058","account_number":"0123456789"}"#;
    let status = raw_send(app, "POST", "/withdraw", Some("text/plain"), body_str).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn withdraw_post_with_application_xml_returns_415() {
    let Some(app) = app().await else {
        return;
    };
    let body_str = r#"{"amount_stroops":500000000,"bank_code":"058","account_number":"0123456789"}"#;
    let status = raw_send(app, "POST", "/withdraw", Some("application/xml"), body_str).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn withdraw_post_with_no_content_type_but_body_returns_415() {
    let Some(app) = app().await else {
        return;
    };
    let body_str = r#"{"amount_stroops":500000000,"bank_code":"058","account_number":"0123456789"}"#;
    let status = raw_send(app, "POST", "/withdraw", None, body_str).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

// ── Verify JSON still passes through ──────────────────────────────────────────

#[tokio::test]
async fn login_with_json_content_type_is_not_rejected_by_middleware() {
    let Some(app) = app().await else {
        return;
    };
    // This request has a valid Content-Type. The middleware must pass it through.
    // The handler will likely return 401 (bad credentials) but NOT 415.
    let body_str = r#"{"email":"nobody@example.com","password":"password123"}"#;
    let status = raw_send(
        app,
        "POST",
        "/login",
        Some("application/json"),
        body_str,
    )
    .await;
    assert_ne!(
        status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "application/json must pass the content-type middleware"
    );
}
