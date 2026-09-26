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

pub async fn state() -> Option<AppState> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        return None;
    };
    let db = match PgPoolOptions::new().max_connections(5).connect(&url).await {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("TEST_DATABASE_URL could not be reached: {err}");
            return None;
        }
    };

    let _guard = MIGRATION_LOCK.lock().unwrap();
    if !MIGRATED.swap(true, Ordering::SeqCst) {
        sqlx::migrate!()
            .run(&db)
            .await
            .expect("migrations failed");
    }
    drop(_guard);

    Some(AppState {
        db,
        jwt_secret: aframp::SecretString::new("integration-test-secret".into()),
        webhook_secret: aframp::SecretString::new("integration-test-webhook".into()),
        wallet_encryption_key: Arc::new([7u8; 32]),
        payment_provider: Arc::new(aframp::payments::mock::MockProvider),
        otp_provider: Arc::new(aframp::otp::mock::MockOtpProvider),
        otp_hmac_secret: aframp::SecretString::new("integration-test-otp-secret".into()),
        cookie: aframp::CookieConfig {
            secure: true,
            same_site: aframp::SameSite::Lax,
        },
        cngn_issuer: None,
    })
}

pub async fn send(
    app: Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
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
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
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
            "password": "password123",
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
