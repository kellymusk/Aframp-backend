//! Shared setup for the end-to-end flow tests in this directory
//! (`auth_flow.rs`, `wallet_flow.rs`, `payment_request_flow.rs`,
//! `withdrawal_flow.rs`).
//!
//! **Migrations, once per process.** [`state()`] runs `sqlx::migrate!()`
//! exactly once (guarded by [`MIGRATED`]/[`MIGRATION_LOCK`]) the first time
//! any test calls it, then hands out a pool to a database that already has
//! the full schema. Every test file in this crate shares one `cargo test`
//! process, so this only runs once per `cargo test` invocation, not once per
//! test.
//!
//! **No per-test isolation.** There is currently no schema-per-test,
//! transactional rollback, or truncation between tests: every test that
//! calls [`state()`] shares one physical database and its rows persist
//! across tests. That is why every helper that creates a merchant
//! ([`ensure_merchant`]) generates a fresh random email — tests avoid
//! collisions by never reusing identity, not by the database resetting
//! itself. A test that lists or counts rows scoped to something other than
//! its own freshly created merchant/wallet/etc. will observe leftovers from
//! every other test that has run against the same database.
//!
//! **Parallelism.** Rust runs test functions concurrently by default
//! (`cargo test -- --test-threads=N`). That is safe here specifically
//! *because* of the point above — tests only assert against data scoped to
//! identifiers they just created — but it means adding a test that queries
//! unscoped state (e.g. "assert exactly one payment exists") would be a race
//! against every other test in the suite, not a bug in the runner.
//!
//! **No teardown.** Nothing here deletes rows after a test runs. The target
//! database (`TEST_DATABASE_URL`) is treated as disposable and expected to
//! accumulate rows across runs; drop and recreate it if that accumulation
//! ever matters (e.g. before a run that asserts on total row counts).
//!
//! **No silent skip.** [`state()`] panics with an explanatory message when
//! `TEST_DATABASE_URL` is unset or unreachable, so a run without a database
//! fails loudly instead of reporting a false green.

// Each test binary only uses a subset of these helpers.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use aframp::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

static MIGRATION_LOCK: Mutex<()> = Mutex::new(());
static MIGRATED: AtomicBool = AtomicBool::new(false);

/// Why the integration-test database couldn't be set up. Returned instead of
/// silently skipping, so a misconfigured run fails loudly rather than
/// reporting a false green.
#[derive(Debug)]
pub enum TestDbError {
    /// `TEST_DATABASE_URL` isn't set at all.
    MissingUrl,
    /// It's set, but the database couldn't be reached.
    Unreachable(sqlx::Error),
}

impl std::fmt::Display for TestDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TestDbError::MissingUrl => write!(
                f,
                "TEST_DATABASE_URL is not set — integration tests need a dedicated Postgres \
                 database (see CONTRIBUTING.md, \"Running tests\")"
            ),
            TestDbError::Unreachable(err) => {
                write!(f, "TEST_DATABASE_URL is set but could not be reached: {err}")
            }
        }
    }
}

/// Connects to `TEST_DATABASE_URL`, runs migrations once per test binary, and
/// builds an `AppState` wired to the mock providers. Returns an error rather
/// than `None` so callers can't mistake "no database" for "nothing to test".
pub async fn try_state() -> Result<AppState, TestDbError> {
    let url = std::env::var("TEST_DATABASE_URL").map_err(|_| TestDbError::MissingUrl)?;
    let db = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .map_err(TestDbError::Unreachable)?;

    let _guard = MIGRATION_LOCK.lock().unwrap();
    if !MIGRATED.swap(true, Ordering::SeqCst) {
        sqlx::migrate!()
            .run(&db)
            .await
            .expect("migrations failed");
    }
    drop(_guard);

    Ok(AppState {
        db,
        jwt_secret: aframp::SecretString::new("integration-test-secret".into()),
        webhook_secret: aframp::SecretString::new("integration-test-webhook".into()),
        wallet_encryption_key: Arc::new(zeroize::Zeroizing::new([7u8; 32])),
        payment_provider: Arc::new(aframp::payments::mock::MockProvider),
        otp_provider: Arc::new(aframp::otp::mock::MockOtpProvider),
        otp_hmac_secret: aframp::SecretString::new("integration-test-otp-secret".into()),
        cookie: aframp::CookieConfig {
            secure: true,
            same_site: aframp::SameSite::Lax,
        },
        admin_events: tokio::sync::broadcast::channel(16).0,
        daily_withdrawal_limit_stroops: None,
    })
}

/// [`try_state`], failing the calling test if the database isn't configured.
/// Integration tests never skip: a run without a database is a failed run.
pub async fn state() -> AppState {
    try_state().await.unwrap_or_else(|err| panic!("{err}"))
}

pub async fn send(
    app: Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, json, _) = send_with_response_headers(app, method, uri, token, body).await;
    (status, json)
}

/// Like [`send`], but also returns response headers (e.g. for Cache-Control checks).
pub async fn send_with_response_headers(
    app: Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value, axum::http::HeaderMap) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = builder
        .header("content-type", "application/json")
        .body(match body {
            Some(json) => Body::from(serde_json::to_vec(&json).unwrap()),
            None => Body::empty(),
        })
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json, headers)
}

/// Like [`send`], but authenticates with a `Cookie` header the way a browser
/// does and hands back the response's `Set-Cookie` values.
pub async fn send_with_cookie(
    app: Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value, Vec<String>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    let request = builder
        .header("content-type", "application/json")
        .body(match body {
            Some(json) => Body::from(serde_json::to_vec(&json).unwrap()),
            None => Body::empty(),
        })
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let set_cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_string)
        .collect();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json, set_cookie)
}

/// Signs up, pulls the OTP the mock provider "sent" back out, and verifies
/// it — the full round trip, since signup alone no longer returns a
/// session. Every caller of `ensure_merchant` keeps working unchanged.
pub async fn ensure_merchant(app: &Router, seed: &str) -> (String, String) {
    let unique = uuid::Uuid::new_v4();
    let email = format!("{seed}+{}@example.com", unique.simple());
    // A fresh, all-digits local number per call — collision-free across the
    // whole test suite the same way the email's uuid suffix already is.
    let local_digits = (unique.as_u128() as u64) % 10_000_000_000;
    let phone_number = format!("0{local_digits:010}");
    let normalized_phone = format!("+234{:010}", local_digits);

    let (status, json) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(serde_json::json!({
            "email": email,
            "password": "Password123!",
            "name": "Test Merchant",
            "phone_number": phone_number,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "signup failed: {json}");
    let challenge_id = json["challenge_id"]
        .as_str()
        .expect("signup should return a challenge_id")
        .to_string();

    let message = aframp::otp::mock::last_message_for(&normalized_phone)
        .expect("mock otp provider should have recorded a message for this phone");
    let code = extract_otp_code(&message);

    let (status, json) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(serde_json::json!({ "challenge_id": challenge_id, "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "verify-otp failed: {json}");
    (
        json["token"].as_str().unwrap().to_string(),
        json["merchant_id"].as_str().unwrap().to_string(),
    )
}

/// Pulls the 6-digit code out of a mock-provider message body, ignoring
/// surrounding punctuation (e.g. the trailing period after the code).
pub fn extract_otp_code(message: &str) -> String {
    message
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_ascii_digit()))
        .find(|word| word.len() == 6 && word.chars().all(|c| c.is_ascii_digit()))
        .expect("message should contain a 6-digit code")
        .to_string()
}
