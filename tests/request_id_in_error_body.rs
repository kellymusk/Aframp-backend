//! #1467 — `X-Request-ID` appears in the JSON body of every error response.
//!
//! The `inject_request_id` middleware reads the `x-request-id` header that
//! `SetRequestIdLayer` stamps on each inbound request and rewrites every 4xx/5xx
//! JSON body to include a `request_id` field, so a merchant can quote it in a
//! support ticket without knowing to look at response headers.
//!
//! These tests drive the full tower stack (router + middleware layers) in-process
//! using `tower::ServiceExt::oneshot`. Because the error paths under test are
//! triggered by auth rejection (before any DB query), no live database is needed.

mod common;

use aframp::{middleware::SanitizingRequestId, router};
use axum::body::Body;
use axum::http::{HeaderName, Request, StatusCode};
use tower::ServiceExt;
use tower_http::request_id::{PropagateRequestIdLayer, SetRequestIdLayer};

/// Wraps the application router with the same `SetRequestIdLayer` /
/// `PropagateRequestIdLayer` stack used in `main.rs`, so `inject_request_id`
/// has an `x-request-id` header to read.
async fn app_with_request_id() -> axum::Router {
    router(common::state().await)
        .layer(SetRequestIdLayer::new(
            HeaderName::from_static("x-request-id"),
            SanitizingRequestId,
        ))
        .layer(PropagateRequestIdLayer::new(HeaderName::from_static(
            "x-request-id",
        )))
}

/// Send a `GET /me` (always a 401 without a token, no DB query) with an
/// optional `X-Request-ID` and return the status, response body, and the
/// `x-request-id` response header value.
async fn send_unauthenticated(
    client_request_id: Option<&str>,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let app = app_with_request_id().await;
    let mut builder = Request::builder().method("GET").uri("/me");
    if let Some(id) = client_request_id {
        builder = builder.header("x-request-id", id);
    }
    let req = builder.body(Body::empty()).unwrap();
    let res = app.oneshot(req).await.unwrap();

    let status = res.status();
    let response_id = res
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(res.into_body(), 64 * 1024)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json, response_id)
}

// ── request_id present in error body ─────────────────────────────────────────

/// A client-supplied valid UUID is echoed in the error body.
#[tokio::test]
async fn client_supplied_uuid_appears_in_error_body() {
    let client_id = "550e8400-e29b-41d4-a716-446655440000";
    let (status, body, _) = send_unauthenticated(Some(client_id)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "GET /me without auth must be 401: {body}"
    );
    assert_eq!(
        body["request_id"].as_str(),
        Some(client_id),
        "client-supplied UUID must appear in error body: {body}"
    );
}

/// When no `X-Request-ID` is provided, `SanitizingRequestId` generates a
/// fresh UUID, which must appear in the error body.
#[tokio::test]
async fn generated_request_id_appears_in_error_body() {
    let (status, body, response_header_id) = send_unauthenticated(None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let body_id = body["request_id"]
        .as_str()
        .expect("request_id must be present in error body");
    // The body ID must be a valid UUID (i.e. one we generated, not garbage).
    uuid::Uuid::parse_str(body_id).expect("request_id in body must be a valid UUID");
    // The body ID and response header ID must agree.
    assert_eq!(
        Some(body_id),
        response_header_id.as_deref(),
        "request_id in body must match X-Request-ID response header"
    );
}

/// A malformed (non-UUID) `X-Request-ID` from the client is replaced with a
/// generated UUID; that UUID appears in the error body.
#[tokio::test]
async fn malformed_client_id_is_replaced_and_appears_in_error_body() {
    let (status, body, response_header_id) =
        send_unauthenticated(Some("not-a-uuid!!!")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let body_id = body["request_id"]
        .as_str()
        .expect("request_id must be present in error body");
    assert_ne!(body_id, "not-a-uuid!!!", "malformed id must be replaced");
    uuid::Uuid::parse_str(body_id).expect("replaced id must be a valid UUID");
    assert_eq!(
        Some(body_id),
        response_header_id.as_deref(),
        "body request_id must match response header"
    );
}

// ── request_id absent from success body ──────────────────────────────────────

/// `GET /health` succeeds; its body must not have a `request_id` field.
#[tokio::test]
async fn success_response_does_not_include_request_id() {
    let app = app_with_request_id().await;
    let req = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);

    assert_eq!(status, StatusCode::OK);
    assert!(
        body.get("request_id").is_none(),
        "success response must not include request_id: {body}"
    );
}

// ── body and header agree ─────────────────────────────────────────────────────

/// The `request_id` in the error body must always equal the `X-Request-ID`
/// response header value.
#[tokio::test]
async fn body_request_id_matches_response_header_for_error() {
    for client_id in [Some("123e4567-e89b-12d3-a456-426614174000"), None] {
        let (status, body, header_id) = send_unauthenticated(client_id).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            body["request_id"].as_str(),
            header_id.as_deref(),
            "body request_id must match X-Request-ID header (client_id={client_id:?}): {body}"
        );
    }
}
