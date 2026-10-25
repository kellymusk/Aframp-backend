//! Tests for `Retry-After` header on `429 Too Many Requests` responses.
//!
//! **Issue #1273** — The OTP service returns `429` when a client exceeds the
//! resend cooldown (60 s) or the hourly cap (5 sends/hour).  RFC 6585 §4
//! recommends including a `Retry-After` header so clients know exactly how
//! long to wait.
//!
//! These tests drive the API at the HTTP layer and verify the *observable*
//! contract: a `429` on `/signup` or `/login` must include a `Retry-After`
//! response header whose value is a non-negative integer ≤ 60.

mod common;

use aframp::router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Sends a raw POST to `uri` with the given JSON body, returns status + the
/// raw `Retry-After` header value (if present).
async fn post_raw(
    app: axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, Option<String>) {
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("content-length", body_bytes.len().to_string())
        .body(Body::from(body_bytes))
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    (status, retry_after)
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// A `429` on `/signup` (within the 60 s resend cooldown) must include a
/// `Retry-After` header that parses as a non-negative integer ≤ 60.
#[tokio::test]
async fn signup_429_includes_retry_after_header() {
    let state = common::state().await;
    let app = router(state);

    let unique = uuid::Uuid::new_v4();
    let email = format!("retry-after-test+{}@example.com", unique.simple());
    // Produce a deterministic 10-digit local number from the UUID.
    let local_digits = (unique.as_u128() as u64) % 10_000_000_000;
    let phone = format!("0{local_digits:010}");

    let signup_body = serde_json::json!({
        "email": email,
        "password": "Password123!",
        "name": "Retry-After Test",
        "phone_number": phone,
    });

    // First call — should succeed (200 with a challenge_id).
    let (status, _) = post_raw(app.clone(), "/signup", signup_body.clone()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "first signup should return 200 (challenge)"
    );

    // Second call within the cooldown window — must be 429.
    let (status, retry_after) = post_raw(app.clone(), "/signup", signup_body.clone()).await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "rapid re-signup should return 429"
    );

    // ── The feature under test ────────────────────────────────────────────────
    let retry_after_value = retry_after.expect("429 response must include a Retry-After header ");
    let secs: u64 = retry_after_value
        .trim()
        .parse()
        .expect("Retry-After must be a non-negative integer");
    assert!(
        secs <= 60,
        "Retry-After ({secs}) must be ≤ RESEND_COOLDOWN_SECS (60)"
    );
}

/// A `429` on `/login` (within the 60 s resend cooldown) must include a
/// `Retry-After` header.
#[tokio::test]
async fn login_429_includes_retry_after_header() {
    let state = common::state().await;
    let app = router(state);

    // Register a full account so /login has a real user to rate-limit against.
    let _unique = uuid::Uuid::new_v4();
    // A verified account; ensure_merchant signs up with Password123!.
    let (_token, merchant_id) = common::ensure_merchant(&app, "retry-after-login").await;
    let email: String = sqlx::query_scalar(
        "SELECT u.email FROM users u JOIN merchants m ON m.user_id = u.id WHERE m.id = $1::uuid",
    )
    .bind(&merchant_id)
    .fetch_one(&common::state().await.db)
    .await
    .unwrap();

    let login_body = serde_json::json!({ "email": email, "password": "Password123!" });

    // First login call — should succeed (200 with challenge_id since account
    // has a phone_number).
    let (status, _) = post_raw(app.clone(), "/login", login_body.clone()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "first login should succeed with 200 (OTP challenge)"
    );

    // Immediate retry — should be 429 (cooldown).
    let (status, retry_after) = post_raw(app.clone(), "/login", login_body).await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "rapid re-login should return 429"
    );

    let retry_after_value = retry_after.expect("429 login response must include Retry-After ");
    let secs: u64 = retry_after_value
        .trim()
        .parse()
        .expect("Retry-After must be a non-negative integer");
    assert!(secs <= 60, "Retry-After ({secs}) must be ≤ 60");
}

/// Non-429 error responses must NOT include a `Retry-After` header.
#[tokio::test]
async fn non_429_responses_have_no_retry_after() {
    let state = common::state().await;
    let app = router(state);

    // 401 — wrong password
    let (status, retry_after) = post_raw(
        app.clone(),
        "/login",
        serde_json::json!({ "email": "nonexistent@example.com", "password": "wrongpass" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        retry_after.is_none(),
        "401 response must not include Retry-After, got: {retry_after:?}"
    );

    // 4xx — missing fields
    let (status, retry_after) = post_raw(
        app.clone(),
        "/signup",
        serde_json::json!({ "email": "x@example.com" }),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    assert!(
        retry_after.is_none(),
        "4xx response must not include Retry-After, got: {retry_after:?}"
    );
}
