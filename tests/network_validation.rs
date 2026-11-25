mod common;

use axum::http::StatusCode;
use serde_json::json;
use std::sync::Mutex;

use common::{ensure_merchant, send, state};

/// Serializes tests that mutate process-wide environment variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Sets the minimal environment required for `AppConfig::from_env()` to succeed,
/// plus `CNGNX_ISSUER_ADDRESS` when provided. Returns a guard that restores the
/// previous values on drop.
struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn new(issuer: Option<&str>) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let keys = [
            "JWT_SECRET",
            "WEBHOOK_SECRET",
            "CNGNX_ISSUER_ADDRESS",
        ];
        let saved = keys
            .iter()
            .map(|k| (*k, std::env::var(k).ok()))
            .collect::<Vec<_>>();

        std::env::set_var("JWT_SECRET", "test-jwt-secret");
        std::env::set_var("WEBHOOK_SECRET", "test-webhook-secret");
        match issuer {
            Some(v) => std::env::set_var("CNGNX_ISSUER_ADDRESS", v),
            None => std::env::remove_var("CNGNX_ISSUER_ADDRESS"),
        }

        Self { _lock: lock, saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[test]
fn rejects_invalid_cngnx_issuer_address() {
    let _guard = EnvGuard::new(Some("not-a-stellar-address"));

    let err = aframp::config::AppConfig::from_env()
        .expect_err("invalid Stellar account ID should be rejected");
    assert!(
        err.to_string().contains("Stellar account ID"),
        "unexpected error: {err}"
    );
}

#[test]
fn accepts_valid_cngnx_issuer_address() {
    // A syntactically valid Stellar G-address (56-char strkey).
    let valid = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
    let _guard = EnvGuard::new(Some(valid));

    let config = aframp::config::AppConfig::from_env()
        .expect("valid Stellar account ID should be accepted");
    assert!(config.cngn_issuer_address.is_some());
}

#[test]
fn empty_cngnx_issuer_address_is_treated_as_unset() {
    let _guard = EnvGuard::new(Some(""));

    let config = aframp::config::AppConfig::from_env()
        .expect("empty Stellar account ID should be treated as unset");
    assert!(config.cngn_issuer_address.is_none());
}

async fn app() -> axum::Router {
    aframp::router(state().await)
}

#[tokio::test]
async fn rejects_unknown_network_with_field_error() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "netvalidate-unknown").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({ "network": "ethereum" })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["field"], "network");
}

#[tokio::test]
async fn rejects_empty_network_with_field_error() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "netvalidate-empty").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({ "network": "" })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["field"], "network");
}

#[tokio::test]
async fn accepts_stellar_network() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "netvalidate-stellar").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({ "network": "stellar" })),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{json}");
}

#[tokio::test]
async fn wallet_create_accepts_an_empty_body() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "netvalidate-empty-body").await;
    let (status, json) = send(app.clone(), "POST", "/wallet/create", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["network"], "stellar");
}

#[tokio::test]
async fn wallet_create_rejects_malformed_json() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "netvalidate-bad-json").await;
    let request = axum::http::Request::post("/wallet/create")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{not json"))
        .unwrap();
    let response = tower::ServiceExt::oneshot(app, request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
