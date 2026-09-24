mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{send, state};

async fn app() -> Option<axum::Router> {
    state().await.map(aframp::router)
}

#[tokio::test]
async fn termii_webhook_valid_signature_acknowledged() {
    let Some(app) = app().await else {
        return;
    };
    let (status, _) = send_with_signature(
        &app,
        "mock-signature",
        json!({
            "type": "sms",
            "message_id": "abc123",
            "receiver": "+2348011122233",
            "status": "Delivered",
            "channel": "dnd"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn termii_webhook_wrong_signature_rejected() {
    let Some(app) = app().await else {
        return;
    };
    let (status, body) = send_with_signature(
        &app,
        "not-the-real-signature",
        json!({ "message_id": "abc123", "status": "Delivered" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "FORBIDDEN");
}

#[tokio::test]
async fn termii_webhook_missing_signature_rejected() {
    let Some(app) = app().await else {
        return;
    };
    let (status, _) = send(app.clone(), "POST", "/webhooks/termii", None, Some(json!({ "status": "Delivered" }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn termii_webhook_unrecognized_shape_still_acknowledged() {
    let Some(app) = app().await else {
        return;
    };
    // Signature is valid but the payload doesn't look like anything we
    // expect — should still ack, never train the provider to retry forever.
    let (status, _) = send_with_signature(&app, "mock-signature", json!({ "totally": "unexpected" })).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

async fn send_with_signature(
    app: &axum::Router,
    signature: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let request = Request::builder()
        .method("POST")
        .uri("/webhooks/termii")
        .header("content-type", "application/json")
        .header("x-termii-signature", signature)
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}
