//! Tests for issue #1476: login rate-limit accounting and email enumeration
//! protection.
//!
//! These tests document and lock in two security properties of the login
//! flow:
//!
//! 1. `login_rate_limit::hit` accumulates attempts uniformly regardless of
//!    whether the supplied email exists. An attacker must not be able to
//!    distinguish valid from invalid emails by observing how quickly the
//!    rate limit trips.
//! 2. The HTTP-level response for an "unknown email" is indistinguishable
//!    from a "wrong password" response (same status, same body shape, and a
//!    dummy hash is computed on the unknown-email path so timing does not
//!    leak account existence).

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::common;

/// Number of attempts used to exercise the rate limiter. Kept in sync with
/// the limiter's configured threshold so the test trips it deterministically.
const ATTEMPTS: usize = 100;

/// A syntactically valid email that is guaranteed not to exist in the test
/// database.
const UNKNOWN_EMAIL: &str = "definitely-not-a-user-1476@example.invalid";

/// A valid email seeded by the test fixtures.
const KNOWN_EMAIL: &str = "user@example.com";

/// A password that is guaranteed to be wrong for `KNOWN_EMAIL`.
const WRONG_PASSWORD: &str = "not-the-right-password-1476";

fn login_body(email: &str, password: &str) -> Body {
    Body::from(
        serde_json::json!({
            "email": email,
            "password": password,
        })
        .to_string(),
    )
}

fn login_request(email: &str, password: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(login_body(email, password))
        .expect("build login request")
}

/// Returns the index of the first attempt that was rate limited, or `None`
/// if the limiter never tripped within `ATTEMPTS` tries.
async fn first_rate_limited_attempt(app: axum::Router, email: &str, password: &str) -> Option<usize> {
    for attempt in 0..ATTEMPTS {
        let response = app
            .clone()
            .oneshot(login_request(email, password))
            .await
            .expect("login request should complete");

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Some(attempt);
        }
    }
    None
}

/// Issue #1476: the rate-limit counter must accumulate identically for
/// unknown and known emails. If the limiter only counted valid emails (or
/// counted them differently), the thresholds below would diverge and an
/// attacker could enumerate accounts.
#[tokio::test]
async fn rate_limit_threshold_is_identical_for_unknown_and_known_email() {
    let unknown_app = common::test_app().await;
    let known_app = common::test_app().await;

    let unknown_threshold = first_rate_limited_attempt(unknown_app, UNKNOWN_EMAIL, WRONG_PASSWORD).await;
    let known_threshold = first_rate_limited_attempt(known_app, KNOWN_EMAIL, WRONG_PASSWORD).await;

    assert_eq!(
        unknown_threshold, known_threshold,
        "rate limit must trip at the same attempt count for unknown and known emails \
         (unknown={unknown_threshold:?}, known={known_threshold:?})"
    );

    assert!(
        unknown_threshold.is_some(),
        "rate limit should trip within {ATTEMPTS} attempts for an unknown email"
    );
}

/// Issue #1476: the HTTP response for an unknown email must be
/// indistinguishable from the response for a known email with a wrong
/// password. Both should return the same status code and the same generic
/// error body, and the unknown-email path must still perform the dummy hash
/// so that response timing does not leak account existence.
#[tokio::test]
async fn unknown_email_and_wrong_password_are_indistinguishable() {
    let unknown_app = common::test_app().await;
    let known_app = common::test_app().await;

    let unknown_start = Instant::now();
    let unknown_response = unknown_app
        .oneshot(login_request(UNKNOWN_EMAIL, WRONG_PASSWORD))
        .await
        .expect("unknown-email login should complete");
    let unknown_elapsed = unknown_start.elapsed();

    let known_start = Instant::now();
    let known_response = known_app
        .oneshot(login_request(KNOWN_EMAIL, WRONG_PASSWORD))
        .await
        .expect("wrong-password login should complete");
    let known_elapsed = known_start.elapsed();

    assert_eq!(
        unknown_response.status(),
        known_response.status(),
        "unknown email and wrong password must return the same status code"
    );
    assert_eq!(
        unknown_response.status(),
        StatusCode::UNAUTHORIZED,
        "failed logins should return 401 Unauthorized"
    );

    let unknown_body = axum::body::to_bytes(unknown_response.into_body(), usize::MAX)
        .await
        .expect("read unknown-email body");
    let known_body = axum::body::to_bytes(known_response.into_body(), usize::MAX)
        .await
        .expect("read wrong-password body");

    assert_eq!(
        unknown_body, known_body,
        "unknown email and wrong password must return an identical error body"
    );

    // Timing check: the unknown-email path must run the dummy hash so that
    // it is not measurably faster than the wrong-password path. We allow a
    // generous slack to avoid flakiness on shared CI runners while still
    // catching a missing dummy hash (which would make the unknown path
    // orders of magnitude faster).
    let slack = Duration::from_millis(50);
    let slower = unknown_elapsed.max(known_elapsed);
    let faster = unknown_elapsed.min(known_elapsed);
    assert!(
        slower.saturating_sub(faster) <= slack,
        "unknown-email and wrong-password timings should be comparable \
         (unknown={unknown_elapsed:?}, known={known_elapsed:?}); a large gap \
         suggests the dummy hash is not being computed on the unknown-email path"
    );
}
