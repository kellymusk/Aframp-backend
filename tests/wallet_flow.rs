mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, state};

async fn app() -> axum::Router {
    aframp::router(state().await)
}

#[tokio::test]
async fn protected_routes_require_token() {
    let app = app().await;
    for (method, path) in [
        ("POST", "/wallet/create"),
        ("GET", "/wallet"),
        ("GET", "/balance"),
        ("GET", "/transactions"),
        ("POST", "/withdraw"),
        ("GET", "/withdrawals"),
    ] {
        let (status, json) = send(app.clone(), method, path, None, None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "expected 401 for {method} {path}: {json}"
        );
    }
}

#[tokio::test]
async fn create_and_fetch_wallet() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "wallet").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create wallet failed: {json}");
    let address = json["address"].as_str().unwrap().to_string();
    assert!(!address.is_empty());
    assert_eq!(json["network"], "stellar");

    let (status, json) = send(app.clone(), "GET", "/wallet", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "get wallet failed: {json}");
    assert_eq!(json["address"], address);
}

#[tokio::test]
async fn get_wallet_returns_404_when_not_created() {
    let Some(app) = app().await else {
        return;
    };
    let (token, _) = ensure_merchant(&app, "wallet_missing").await;

    let (status, json) = send(app.clone(), "GET", "/wallet", Some(&token), None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "expected 404 when wallet is missing: {json}"
    );
    assert_eq!(json["code"], "WALLET_NOT_FOUND");
}

#[tokio::test]
async fn balance_and_transactions_start_empty() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "empty").await;

    let (status, json) = send(app.clone(), "GET", "/balance", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "balance failed: {json}");
    assert_eq!(json, json!([]));

    let (status, json) = send(app.clone(), "GET", "/transactions", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "transactions failed: {json}");
    assert_eq!(json, json!([]));
}

#[tokio::test]
async fn wallet_address_is_stable_per_merchant() {
    let app = app().await;
    let (token_a, _) = ensure_merchant(&app, "stable_a").await;
    let (token_b, _) = ensure_merchant(&app, "stable_b").await;

    send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token_a),
        Some(json!({})),
    )
    .await;
    send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token_b),
        Some(json!({})),
    )
    .await;

    let (_, json_a) = send(app.clone(), "GET", "/wallet", Some(&token_a), None).await;
    let (_, json_b) = send(app.clone(), "GET", "/wallet", Some(&token_b), None).await;
    assert_ne!(json_a["address"], json_b["address"]);
}

#[tokio::test]
async fn backed_off_wallet_is_excluded_from_poll_query() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "wallet_backoff").await;

    let (status, json) = send(
// ─────────────────────────────────────────────────────────────────────────────
// #1042 — duplicate wallet prevention: second POST /wallet/create must fail
// ─────────────────────────────────────────────────────────────────────────────

/// POST /wallet/create is intentionally limited to one wallet per merchant.
///
/// The `wallets_merchant_id_unique` constraint (migration 0009) enforces this
/// at the database level.  A second call must return 409 Conflict rather than
/// silently creating a second wallet that would be unreachable (the service
/// returns the *newest* by `created_at DESC`, effectively orphaning the first
/// and any funds held in it).
#[tokio::test]
async fn second_wallet_create_returns_409() {
    let Some(app) = app().await else {
        return;
    };
    let (token, _) = ensure_merchant(&app, "dup_wallet").await;

    // First call succeeds and returns the new wallet.
    let (status, first) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create wallet failed: {json}");
    let address = json["address"].as_str().unwrap().to_string();

    let polled = aframp::services::wallets::pollable_wallets(&state.db, &[])
        .await
        .unwrap();
    assert!(polled.iter().any(|w| w.address == address));

    let polled =
        aframp::services::wallets::pollable_wallets(&state.db, std::slice::from_ref(&address))
            .await
            .unwrap();
    assert!(
        !polled.iter().any(|w| w.address == address),
        "a wallet in backoff must not be loaded for polling"
    assert_eq!(status, StatusCode::OK, "first create should succeed: {first}");
    let first_address = first["address"].as_str().unwrap().to_string();
    assert!(!first_address.is_empty());

    // Second call for the same merchant must be rejected.
    let (status, second) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "second create should return 409: {second}"
    );

    // The existing wallet is unchanged — GET /wallet still returns the original.
    let (status, fetched) = send(app.clone(), "GET", "/wallet", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "get wallet failed: {fetched}");
    assert_eq!(
        fetched["address"], first_address,
        "existing wallet address must be preserved after a rejected duplicate create"
    );
}
