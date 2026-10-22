//! Integration tests for email validation through the `/signup` endpoint.
//!
//! `is_valid_email` in `src/validation.rs` is tested indirectly here because
//! it is a private module — the public surface is the `/signup` handler that
//! calls it. Every assertion below maps to a specific validation case listed
//! in issue #1104.
//!
//! Run these tests with:
//!
//! ```bash
//! TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/aframp_test \
//!   cargo test --test email_validation_tests
//! ```

mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::state;

async fn app() -> axum::Router {
    aframp::router(state().await)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A fresh valid Nigerian phone number per signup, so the per-number OTP
/// send limit never turns a validation result into a 429.
fn fresh_phone() -> String {
    let n = (uuid::Uuid::new_v4().as_u128() % 100_000_000) as u64;
    format!("080{n:08}")
}

/// POST /signup with the given email. Returns the HTTP status code.
/// Other fields are fixed — we're only varying the email.
async fn signup_status(app: axum::Router, email: &str) -> StatusCode {
    let (status, _body) = common::send(
        app,
        "POST",
        "/signup",
        None,
        Some(json!({
            "email": email,
            "password": "ValidPass123!",
            "name": "Test User",
            "phone_number": fresh_phone(),
        })),
    )
    .await;
    status
}

// ---------------------------------------------------------------------------
// Valid email formats — should be accepted (HTTP 200 OK)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn accepts_ng_tld() {
    let app = app().await;
    let status = signup_status(app, "merchant@example.ng").await;
    assert_eq!(
        status,
        StatusCode::OK,
        ".ng TLD must be accepted by is_valid_email"
    );
}

#[tokio::test]
async fn accepts_co_uk_tld() {
    let app = app().await;
    let status = signup_status(app, "merchant@example.co.uk").await;
    assert_eq!(
        status,
        StatusCode::OK,
        ".co.uk TLD must be accepted by is_valid_email"
    );
}

#[tokio::test]
async fn accepts_io_tld() {
    let app = app().await;
    let status = signup_status(app, "merchant@startup.io").await;
    assert_eq!(
        status,
        StatusCode::OK,
        ".io TLD must be accepted by is_valid_email"
    );
}

#[tokio::test]
async fn accepts_tagged_address() {
    let app = app().await;
    // RFC 5321 allows '+' in the local part (common for Gmail-style tagging).
    let status = signup_status(app, "user+tag@example.com").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "tagged address (user+tag@example.com) must be accepted"
    );
}

#[tokio::test]
async fn accepts_subdomain_address() {
    let app = app().await;
    let status = signup_status(app, "user@mail.subdomain.example.com").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "address with subdomain must be accepted"
    );
}

#[tokio::test]
async fn accepts_maximum_length_local_part() {
    let app = app().await;
    // RFC 5321 §4.5.3.1.1 — local part max is 64 characters.
    let local_part = "a".repeat(64);
    let email = format!("{local_part}@example.com");
    let status = signup_status(app, &email).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "64-character local part must be accepted (RFC 5321 limit)"
    );
}

#[tokio::test]
async fn accepts_maximum_length_domain() {
    let app = app().await;
    // RFC 5321 §4.5.3.1.2 — domain max is 255 characters.
    // Build a valid domain that's exactly 255 chars: four 62-char labels + dots + .com
    // "a" * 62 + "." + "a" * 62 + "." + "a" * 62 + "." + "a" * 61 + ".com" = 255
    let domain = format!(
        "{}.{}.{}.{}.com",
        "a".repeat(62),
        "a".repeat(62),
        "a".repeat(62),
        "a".repeat(54),
    );
    assert!(domain.len() <= 255, "test domain must be within 255 chars");
    let email = format!("user@{domain}");
    let status = signup_status(app, &email).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "domain within 255-char limit must be accepted"
    );
}

// ---------------------------------------------------------------------------
// Invalid email formats — should be rejected (HTTP 400 with code INVALID_PARAMETERS)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rejects_missing_at_sign() {
    let app = app().await;
    let status = signup_status(app, "notanemail.com").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "address without '@' must be rejected"
    );
}

#[tokio::test]
async fn rejects_double_at_sign() {
    let app = app().await;
    let status = signup_status(app, "user@@example.com").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "address with double '@' must be rejected"
    );
}

#[tokio::test]
async fn rejects_trailing_dot_in_domain() {
    let app = app().await;
    let status = signup_status(app, "user@example.com.").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "domain with trailing dot must be rejected"
    );
}

#[tokio::test]
async fn rejects_leading_dot_in_local_part() {
    let app = app().await;
    let status = signup_status(app, ".user@example.com").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "local part with leading dot must be rejected"
    );
}

#[tokio::test]
async fn rejects_trailing_dot_in_local_part() {
    let app = app().await;
    let status = signup_status(app, "user.@example.com").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "local part with trailing dot must be rejected"
    );
}

#[tokio::test]
async fn rejects_consecutive_dots_in_local_part() {
    let app = app().await;
    let status = signup_status(app, "user..name@example.com").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "local part with consecutive dots must be rejected"
    );
}

#[tokio::test]
async fn rejects_domain_with_no_dot() {
    let app = app().await;
    // A domain with no dot has no TLD — invalid per the validator.
    let status = signup_status(app, "user@localhost").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "domain without a dot must be rejected"
    );
}

#[tokio::test]
async fn rejects_local_part_exceeding_64_chars() {
    let app = app().await;
    let local_part = "a".repeat(65);
    let email = format!("{local_part}@example.com");
    let status = signup_status(app, &email).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "local part longer than 64 chars must be rejected"
    );
}

#[tokio::test]
async fn rejects_domain_exceeding_255_chars() {
    let app = app().await;
    // 256-character domain — one byte over the limit.
    let domain = format!("{}.example.com", "a".repeat(244));
    assert!(domain.len() > 255, "test domain must exceed 255 chars");
    let email = format!("user@{domain}");
    let status = signup_status(app, &email).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "domain longer than 255 chars must be rejected"
    );
}

#[tokio::test]
async fn rejects_empty_local_part() {
    let app = app().await;
    let status = signup_status(app, "@example.com").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "empty local part must be rejected"
    );
}

#[tokio::test]
async fn rejects_empty_string() {
    let app = app().await;
    let status = signup_status(app, "").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "empty email string must be rejected"
    );
}
