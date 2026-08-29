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
        jwt_secret: Arc::new("integration-test-secret".into()),
        webhook_secret: Arc::new("integration-test-webhook".into()),
        wallet_encryption_key: Arc::new([7u8; 32]),
        payment_provider: Arc::new(aframp::payments::mock::MockProvider),
        cookie: aframp::CookieConfig {
            secure: true,
            same_site: aframp::SameSite::Lax,
        },
        daily_withdrawal_limit_stroops: None,
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

pub async fn ensure_merchant(app: &Router, seed: &str) -> (String, String) {
    let email = format!("{seed}+{}@example.com", uuid::Uuid::new_v4().simple());
    let (status, json) = send(
        app.clone(),
        "POST",
        "/signup",
        None,
        Some(serde_json::json!({
            "email": email,
            "password": "password123",
            "name": "Test Merchant"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "signup failed: {json}");
    (
        json["token"].as_str().unwrap().to_string(),
        json["merchant_id"].as_str().unwrap().to_string(),
    )
}
