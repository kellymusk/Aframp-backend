//! Request middleware shared by the HTTP layer.
//!
//! The `Json` extractor already rejects mislabeled bodies per-handler, but its
//! rejection bypasses the `{ "error": ... }` contract (plain-text body) and
//! never runs at all for bodyless routes. This middleware enforces the
//! documented content-type contract — `application/json` on every request with
//! a body — before any handler runs, and answers with the same JSON error
//! shape as the rest of the API.

use axum::extract::Request;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use axum::Json;
use tower_http::request_id::{MakeRequestId, RequestId};
use uuid::Uuid;

use crate::config::Config;
use crate::error::{unsupported_media_type, ApiError, ErrorCode};

/// `Strict-Transport-Security` value advertised in production. One year,
/// including subdomains, matching the proposal in
/// `docs/proposals/1248-hsts-header.md`.
pub const HSTS_VALUE: &str = "max-age=31536000; includeSubDomains";

/// Generates the `X-Request-ID` correlating one request across logs and a
/// client bug report. An incoming `X-Request-ID` is trusted and echoed back
/// only if it is already a valid UUID — anything else (missing, malformed, or
/// used to try to inject a bogus value into logs) gets a freshly generated
/// one instead, so the header in a log line is always a value we generated or
/// explicitly vetted.
#[derive(Clone, Default)]
pub struct SanitizingRequestId;

impl MakeRequestId for SanitizingRequestId {
    fn make_request_id<B>(&mut self, request: &axum::http::Request<B>) -> Option<RequestId> {
        let sanitized = request
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| Uuid::parse_str(v).ok())
            .map(|id| id.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        HeaderValue::from_str(&sanitized).ok().map(RequestId::new)
    }
}

/// Rejects POST/PUT requests that carry a body but declare a content type
/// other than JSON, so handlers that expect `Json` never see a mislabeled
/// payload. Bodyless POSTs (e.g. `POST /logout`) and every other method pass
/// through untouched.
pub async fn require_json_content_type(
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<ApiError>)> {
    if matches!(req.method(), &Method::POST | &Method::PUT)
        && has_body(&req)
        && !is_json_content_type(req.headers().get(header::CONTENT_TYPE))
    {
        return Err(unsupported_media_type(
            ErrorCode::InvalidParameters,
            "content-type must be application/json",
        ));
    }
    Ok(next.run(req).await)
}

/// A request "has a body" when it is chunked or declares a non-zero
/// `Content-Length`. A missing or zero length means the client sent nothing,
/// and a bodyless POST is a valid way to trigger a side effect (`/logout`).
fn has_body(req: &Request) -> bool {
    if req.headers().contains_key(header::TRANSFER_ENCODING) {
        return true;
    }
    match req.headers().get(header::CONTENT_LENGTH) {
        Some(len) => len
            .to_str()
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(|len| len > 0)
            // A malformed length is a broken request; treat it as carrying a
            // body so the content-type check still applies.
            .unwrap_or(true),
        None => false,
    }
}

/// Mirrors axum's own `Json` extractor: `application/json` or any media type
/// with a `+json` suffix (e.g. `application/vnd.api+json`) is acceptable, and
/// parameters like `charset=utf-8` are ignored. Anything else — including a
/// missing header — is not.
fn is_json_content_type(value: Option<&HeaderValue>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Ok(media_type) = value.parse::<mime::Mime>() else {
        return false;
    };
    media_type.subtype() == mime::JSON || media_type.suffix() == Some(mime::JSON)
}

/// Adds `Strict-Transport-Security` to every response when the service runs
/// in production. The header is only inserted when absent, so a TLS-terminating
/// proxy that already sets its own value wins. Whether production is active is
/// decided once, when the layer is built via [`Hsts::from_config`], rather than
/// reading the environment on every request.
#[derive(Clone, Copy, Debug)]
pub struct Hsts {
    enabled: bool,
}

impl Hsts {
    /// Builds the layer from the resolved application config. Production is
    /// detected from the same signal the rest of the app uses (`APP_ENV`),
    /// exposed as [`Config::is_production`].
    pub fn from_config(config: &Config) -> Self {
        Self {
            enabled: config.is_production(),
        }
    }

    /// Builds the layer from an explicit flag, for tests and callers that
    /// already know the environment.
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Runs the inner handler and, in production, stamps the HSTS header onto
    /// the response unless one is already present.
    pub async fn run(&self, request: Request, next: Next) -> Response {
        let mut response = next.run(request).await;
        if self.enabled && !response.headers().contains_key(header::STRICT_TRANSPORT_SECURITY) {
            response.headers_mut().insert(
                header::STRICT_TRANSPORT_SECURITY,
                HeaderValue::from_static(HSTS_VALUE),
            );
        }
        response
    }
}

/// Middleware form of [`Hsts`] for `axum::middleware::from_fn_with_state`.
pub async fn hsts(
    axum::extract::State(hsts): axum::extract::State<Hsts>,
    request: Request,
    next: Next,
) -> Response {
    hsts.run(request, next).await
/// Injects the `X-Request-ID` for this request into the JSON body of every
/// error response, so a merchant receiving e.g. a 500 can quote the ID in a
/// support ticket without knowing to look at the response headers.
///
/// The middleware reads `x-request-id` from the **request** headers (placed
/// there by `SetRequestIdLayer` before this middleware runs), then — for any
/// response whose status is 4xx or 5xx — deserialises the body, adds the
/// `request_id` field, and re-serialises. Responses that are not JSON, have
/// no body, or whose body is not a valid JSON object are passed through
/// unchanged so non-error routes are never affected.
pub async fn inject_request_id(req: Request, next: Next) -> Response {
    // Capture the ID from the request before we consume it.
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned());

    let response = next.run(req).await;

    // Only rewrite error responses.
    if !response.status().is_client_error() && !response.status().is_server_error() {
        return response;
    }

    // Only rewrite JSON bodies.
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("application/json"))
        .unwrap_or(false);
    if !is_json {
        return response;
    }

    let Some(id) = request_id else {
        return response;
    };

    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 64 * 1024).await else {
        return Response::from_parts(parts, axum::body::Body::empty());
    };

    // Deserialise, inject, re-serialise.
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Response::from_parts(parts, axum::body::Body::from(bytes));
    };
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "request_id".to_string(),
            serde_json::Value::String(id),
        );
    }
    let Ok(new_bytes) = serde_json::to_vec(&value) else {
        return Response::from_parts(parts, axum::body::Body::from(bytes));
    };

    // Update Content-Length to match the new (longer) body.
    if let Ok(len) = HeaderValue::from_str(&new_bytes.len().to_string()) {
        parts.headers.insert(header::CONTENT_LENGTH, len);
    }
    Response::from_parts(parts, axum::body::Body::from(new_bytes))
}

/// Adds a `Retry-After` header to 423/429 responses whose JSON body carries
/// `retry_after_secs`, so every rate-limit and lockout path advertises it
/// (RFC 9110 §10.2.3) without each handler building headers by hand.
pub async fn retry_after_header(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    let status = response.status();
    if (status != StatusCode::TOO_MANY_REQUESTS && status != StatusCode::LOCKED)
        || response.headers().contains_key(header::RETRY_AFTER)
    {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 64 * 1024).await else {
        return Response::from_parts(parts, axum::body::Body::empty());
    };
    let secs = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| {
            v.get("retry_after_secs")
                .and_then(serde_json::Value::as_u64)
        });
    if let Some(secs) = secs {
        parts
            .headers
            .insert(header::RETRY_AFTER, HeaderValue::from(secs));
    }
    Response::from_parts(parts, axum::body::Body::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::{get, post, put};
    use axum::Router;
    use serde_json::Value;
    use tower::ServiceExt;

    async fn handler() -> &'static str {
        "ok"
    }

    async fn error_handler() -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
        (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": "something went wrong",
                "code": "INVALID_PARAMETERS"
            })),
        )
    }

    async fn internal_error_handler() -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({
                "error": "internal server error",
                "code": "INTERNAL_ERROR"
            })),
        )
    }

    fn app() -> Router {
        Router::new()
            .route("/post", post(handler))
            .route("/put", put(handler))
            .route("/get", get(handler))
            .layer(axum::middleware::from_fn(require_json_content_type))
    }

    fn inject_app() -> Router {
        Router::new()
            .route("/ok", get(handler))
            .route("/error", get(error_handler))
            .route("/internal", get(internal_error_handler))
            .layer(axum::middleware::from_fn(inject_request_id))
    }

    /// Builds a request, setting `Content-Length` whenever a body is present
    /// the way a real HTTP client would.
    fn request(method: &str, uri: &str, content_type: Option<&str>, body: &str) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(ct) = content_type {
            builder = builder.header(header::CONTENT_TYPE, ct);
        }
        if !body.is_empty() {
            builder = builder.header(header::CONTENT_LENGTH, body.len().to_string());
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }

    async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1024).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn wrong_content_type_with_a_body_is_415() {
        let app = app();
        let (status, body) =
            send(&app, request("POST", "/post", Some("text/plain"), "hello")).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(body["error"], "content-type must be application/json");

        let (status, _) = send(
            &app,
            request("PUT", "/put", Some("application/xml"), "<x/>"),
        )
        .await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn missing_content_type_with_a_body_is_415() {
        let app = app();
        let (status, body) = send(&app, request("POST", "/post", None, "{}")).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(body["error"], "content-type must be application/json");
    }

    #[tokio::test]
    async fn json_content_types_pass() {
        let app = app();
        for ct in [
            "application/json",
            "application/json; charset=utf-8",
            "application/vnd.api+json",
        ] {
            let (status, _) = send(&app, request("POST", "/post", Some(ct), "{}")).await;
            assert_eq!(status, StatusCode::OK, "rejected {ct}");
        }
    }

    #[tokio::test]
    async fn bodyless_posts_pass_without_a_content_type() {
        let app = app();
        let (status, _) = send(&app, request("POST", "/post", None, "")).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn non_post_methods_are_unaffected() {
        let app = app();
        let (status, _) = send(&app, request("GET", "/get", Some("text/plain"), "hello")).await;
        assert_eq!(status, StatusCode::OK);
    }

    fn hsts_app(enabled: bool) -> Router {
        Router::new()
            .route("/get", get(handler))
            .layer(axum::middleware::from_fn_with_state(
                Hsts::new(enabled),
                hsts,
            ))
    }

    #[tokio::test]
    async fn hsts_header_present_in_production() {
        let app = hsts_app(true);
        let res = app
            .clone()
            .oneshot(request("GET", "/get", None, ""))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::STRICT_TRANSPORT_SECURITY)
                .and_then(|v| v.to_str().ok()),
            Some(HSTS_VALUE)
    // ── inject_request_id tests ───────────────────────────────────────────────

    async fn send_inject(
        app: &Router,
        uri: &str,
        request_id: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method("GET").uri(uri);
        if let Some(id) = request_id {
            builder = builder.header("x-request-id", id);
        }
        let req = builder.body(Body::empty()).unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn error_response_includes_request_id_from_header() {
        let app = inject_app();
        let id = "550e8400-e29b-41d4-a716-446655440000";
        let (status, body) = send_inject(&app, "/error", Some(id)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body["request_id"].as_str(),
            Some(id),
            "request_id must be injected into error body: {body}"
        );
    }

    #[tokio::test]
    async fn hsts_header_absent_in_dev() {
        let app = hsts_app(false);
        let res = app
            .clone()
            .oneshot(request("GET", "/get", None, ""))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(!res
            .headers()
            .contains_key(header::STRICT_TRANSPORT_SECURITY));
    }

    #[tokio::test]
    async fn hsts_header_does_not_override_existing_value() {
        async fn with_hsts() -> Response {
            let mut res = Response::new(axum::body::Body::empty());
            res.headers_mut().insert(
                header::STRICT_TRANSPORT_SECURITY,
                HeaderValue::from_static("max-age=60"),
            );
            res
        }
        let app = Router::new()
            .route("/get", get(with_hsts))
            .layer(axum::middleware::from_fn_with_state(
                Hsts::new(true),
                hsts,
            ));
        let res = app
            .clone()
            .oneshot(request("GET", "/get", None, ""))
            .await
            .unwrap();
        assert_eq!(
            res.headers()
                .get(header::STRICT_TRANSPORT_SECURITY)
                .and_then(|v| v.to_str().ok()),
            Some("max-age=60")
    async fn error_body_retains_existing_fields() {
        let app = inject_app();
        let (status, body) = send_inject(&app, "/error", Some("any-id")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"].as_str(), Some("something went wrong"));
        assert_eq!(body["code"].as_str(), Some("INVALID_PARAMETERS"));
    }

    #[tokio::test]
    async fn internal_error_response_includes_request_id() {
        let app = inject_app();
        let id = "deadbeef-dead-beef-dead-beefdeadbeef";
        let (status, body) = send_inject(&app, "/internal", Some(id)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["request_id"].as_str(), Some(id));
    }

    #[tokio::test]
    async fn success_response_does_not_include_request_id() {
        let app = inject_app();
        let (status, body) = send_inject(&app, "/ok", Some("some-id")).await;
        assert_eq!(status, StatusCode::OK);
        // The success body is a plain string "ok", not JSON with request_id.
        assert!(
            body.get("request_id").is_none(),
            "success body must not have request_id injected: {body}"
        );
    }

    #[tokio::test]
    async fn no_x_request_id_header_leaves_body_unchanged() {
        let app = inject_app();
        let (status, body) = send_inject(&app, "/error", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // No ID on the request → middleware skips injection.
        assert!(
            body.get("request_id").is_none(),
            "without x-request-id header, body must not be rewritten: {body}"
        );
    }
}
