use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::Router;
use tower::ServiceExt;
use tower_http::limit::RequestBodyLimitLayer;

fn app() -> Router {
    Router::new()
        .route(
            "/echo",
            post(|body: Bytes| async move { body.len().to_string() }),
        )
        .layer(RequestBodyLimitLayer::new(
            aframp::DEFAULT_MAX_REQUEST_BODY_BYTES,
        ))
}

async fn post_bytes(len: usize) -> StatusCode {
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header("content-type", "application/json")
        .body(Body::from(vec![b'a'; len]))
        .unwrap();
    app().oneshot(req).await.unwrap().status()
}

#[test]
fn default_limit_is_64_kilobytes() {
    assert_eq!(aframp::DEFAULT_MAX_REQUEST_BODY_BYTES, 64 * 1024);
}

#[tokio::test]
async fn body_at_limit_is_accepted() {
    assert_eq!(
        post_bytes(aframp::DEFAULT_MAX_REQUEST_BODY_BYTES).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn body_over_limit_returns_413() {
    assert_eq!(
        post_bytes(aframp::DEFAULT_MAX_REQUEST_BODY_BYTES + 1).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

/// #1454 — a custom limit is applied, not the default.
#[tokio::test]
async fn custom_limit_rejects_larger_body() {
    let app = Router::new()
        .route(
            "/echo",
            post(|body: Bytes| async move { body.len().to_string() }),
        )
        .layer(RequestBodyLimitLayer::new(1024));
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .body(Body::from(vec![b'a'; 2048]))
        .unwrap();
    assert_eq!(
        app.oneshot(req).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}
