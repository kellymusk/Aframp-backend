//! #1252 — `POST /webhooks/termii` signature verification.
//!
//! Tests run against `MockOtpProvider`, which accepts exactly the sentinel
//! `mock-signature` (see CONTRIBUTING.md, "Mock OTP provider"). Every other
//! value — missing, tampered, or a well-formed HMAC under the wrong key — must
//! be rejected with `403 FORBIDDEN` before the body is looked at. All tests go
//! through `common::state()`, which requires `TEST_DATABASE_URL`.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha512;
use tower::ServiceExt;

use common::state;

const MOCK_SIGNATURE: &str = "mock-signature";

async fn app() -> axum::Router {
    aframp::router(state().await)
}

fn delivery_report() -> Value {
    json!({
        "type": "sms",
        "message_id": "abc123",
        "receiver": "+2348011122233",
        "status": "Delivered",
        "channel": "dnd"
    })
}

/// Hex HMAC-SHA512 of `body` — the format Termii sends in `X-Termii-Signature`.
fn hmac_sha512_hex(key: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha512>::new_from_slice(key.as_bytes()).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// POSTs raw bytes to the Termii webhook, with an optional signature header.
async fn post_termii(app: &axum::Router, signature: Option<&str>, body: Vec<u8>) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/webhooks/termii")
        .header("content-type", "application/json");
    if let Some(signature) = signature {
        builder = builder.header("x-termii-signature", signature);
    }
    let response = app.clone().oneshot(builder.body(Body::from(body)).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn assert_forbidden(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "FORBIDDEN", "{body}");
}

#[tokio::test]
async fn missing_signature_header_is_forbidden() {
    let app = app().await;
    let (status, body) = post_termii(&app, None, serde_json::to_vec(&delivery_report()).unwrap()).await;
    assert_forbidden(status, &body);
}

#[tokio::test]
async fn empty_signature_header_is_forbidden() {
    let app = app().await;
    let (status, body) = post_termii(&app, Some(""), serde_json::to_vec(&delivery_report()).unwrap()).await;
    assert_forbidden(status, &body);
}

#[tokio::test]
async fn valid_payload_with_incorrect_hex_signature_is_forbidden() {
    let app = app().await;
    // Well-formed hex of the right length (SHA-512 = 64 bytes), but wrong.
    let bogus = "ab".repeat(64);
    let (status, body) = post_termii(&app, Some(&bogus), serde_json::to_vec(&delivery_report()).unwrap()).await;
    assert_forbidden(status, &body);
}

#[tokio::test]
async fn correctly_formatted_signature_under_the_wrong_key_is_forbidden() {
    let app = app().await;
    let body = serde_json::to_vec(&delivery_report()).unwrap();
    let signature = hmac_sha512_hex("not-the-webhook-secret", &body);
    let (status, resp) = post_termii(&app, Some(&signature), body).await;
    assert_forbidden(status, &resp);
}

#[tokio::test]
async fn signature_for_a_different_body_is_forbidden() {
    // A signature lifted from one request and replayed on a tampered body.
    let app = app().await;
    let original = serde_json::to_vec(&delivery_report()).unwrap();
    let signature = hmac_sha512_hex("not-the-webhook-secret", &original);
    let mut tampered = delivery_report();
    tampered["status"] = json!("Failed");
    let (status, resp) = post_termii(&app, Some(&signature), serde_json::to_vec(&tampered).unwrap()).await;
    assert_forbidden(status, &resp);
}

#[tokio::test]
async fn near_miss_of_the_mock_sentinel_is_forbidden() {
    // The mock must match the sentinel exactly, not by prefix or case.
    let app = app().await;
    for signature in ["mock-signature ", "MOCK-SIGNATURE", "mock-signatur", "mock-signature-x"] {
        let (status, body) =
            post_termii(&app, Some(signature), serde_json::to_vec(&delivery_report()).unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "signature {signature:?} must be rejected: {body}");
    }
}

#[tokio::test]
async fn mock_sentinel_signature_is_acknowledged() {
    let app = app().await;
    let (status, body) =
        post_termii(&app, Some(MOCK_SIGNATURE), serde_json::to_vec(&delivery_report()).unwrap()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

#[tokio::test]
async fn unrecognized_payload_shape_with_valid_signature_is_acknowledged() {
    // Leniency: once the signature checks out the handler always acks, so
    // Termii is never trained to retry a payload we can't parse.
    let app = app().await;
    let (status, body) =
        post_termii(&app, Some(MOCK_SIGNATURE), serde_json::to_vec(&json!({ "totally": "unexpected" })).unwrap()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

#[tokio::test]
async fn non_json_body_with_valid_signature_is_acknowledged() {
    let app = app().await;
    let (status, body) = post_termii(&app, Some(MOCK_SIGNATURE), b"not json at all".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}
