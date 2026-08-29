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

use crate::error::{unsupported_media_type, ApiError};

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

    fn app() -> Router {
        Router::new()
            .route("/post", post(handler))
            .route("/put", put(handler))
            .route("/get", get(handler))
            .layer(axum::middleware::from_fn(require_json_content_type))
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

    async fn status(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1024).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn wrong_content_type_with_a_body_is_415() {
        let app = app();
        let (status, body) = status(&app, request("POST", "/post", Some("text/plain"), "hello")).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(body["error"], "content-type must be application/json");

        let (status, _) = status(&app, request("PUT", "/put", Some("application/xml"), "<x/>")).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn missing_content_type_with_a_body_is_415() {
        let app = app();
        let (status, body) = status(&app, request("POST", "/post", None, "{}")).await;
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
            let (status, _) = status(&app, request("POST", "/post", Some(ct), "{}")).await;
            assert_eq!(status, StatusCode::OK, "rejected {ct}");
        }
    }

    #[tokio::test]
    async fn bodyless_posts_pass_without_a_content_type() {
        let app = app();
        let (status, _) = status(&app, request("POST", "/post", None, "")).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn non_post_methods_are_unaffected() {
        let app = app();
        let (status, _) = status(&app, request("GET", "/get", Some("text/plain"), "hello")).await;
        assert_eq!(status, StatusCode::OK);
    }
}
