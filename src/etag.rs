use axum::body::Body;
use axum::http::{header, HeaderMap, Response as HttpResponse, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Serializes `body` to JSON, tags it with an ETag derived from a hash of the
/// payload, and returns `304 Not Modified` (no body) when the request's
/// `If-None-Match` already matches — sparing polling frontends the bandwidth
/// of a payload they already have.
pub fn conditional_json<T: Serialize>(headers: &HeaderMap, body: &T) -> Response {
    let payload = match serde_json::to_vec(body) {
        Ok(p) => p,
        Err(err) => {
            tracing::error!(error = %err, "failed to serialize response for etag");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let hash = Sha256::digest(&payload);
    let etag = format!("\"{hash:x}\"");

    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|if_none_match| matches_etag(if_none_match, &etag))
    {
        return HttpResponse::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, etag)
            .body(Body::empty())
            .expect("valid 304 response")
            .into_response();
    }

    HttpResponse::builder()
        .status(StatusCode::OK)
        .header(header::ETAG, etag)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(payload))
        .expect("valid 200 response")
        .into_response()
}

/// `If-None-Match` may carry a comma-separated list of ETags, or `*` to match any.
fn matches_etag(if_none_match: &str, etag: &str) -> bool {
    if if_none_match.trim() == "*" {
        return true;
    }
    if_none_match.split(',').any(|candidate| candidate.trim() == etag)
}
