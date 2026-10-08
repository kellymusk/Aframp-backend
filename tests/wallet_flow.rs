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
async fn balance_and_transactions_start_empty() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "empty").await;

    let (status, json) = send(app.clone(), "GET", "/balance", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "balance failed: {json}");
    assert_eq!(json, json!([]));

    let (status, json) = send(app.clone(), "GET", "/transactions", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "transactions failed: {json}");
    assert_eq!(json["data"], json!([]));
    assert!(json["next_cursor"].is_null());
}

#[tokio::test]
async fn wallet_address_is_stable_per_merchant() {
    let app = app().await;
    let (token_a, _) = ensure_merchant(&app, "stable_a").await;
    let (token_b, _) = ensure_merchant(&app, "stable_b").await;

    send(app.clone(), "POST", "/wallet/create", Some(&token_a), Some(json!({}))).await;
    send(app.clone(), "POST", "/wallet/create", Some(&token_b), Some(json!({}))).await;

    let (_, json_a) = send(app.clone(), "GET", "/wallet", Some(&token_a), None).await;
    let (_, json_b) = send(app.clone(), "GET", "/wallet", Some(&token_b), None).await;
    assert_ne!(json_a["address"], json_b["address"]);
}

#[tokio::test]
async fn get_wallet_returns_404_when_not_created() {
    let app = app().await;
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
async fn backed_off_wallet_is_excluded_from_poll_query() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "wallet_backoff").await;

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
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// #1042 — duplicate wallet prevention: second POST /wallet/create must fail
// ─────────────────────────────────────────────────────────────────────────────

/// A merchant holds at most one wallet per network. The
/// `wallets_merchant_id_network_unique` constraint enforces this at the
/// database level, so a second create must return 409 Conflict rather than
/// silently orphaning the first wallet and any funds held in it.
#[tokio::test]
async fn second_wallet_create_returns_409() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "dup_wallet").await;

    let (status, first) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "first create should succeed: {first}");
    let first_address = first["address"].as_str().unwrap().to_string();
    assert!(!first_address.is_empty());

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

    let (status, fetched) = send(app.clone(), "GET", "/wallet", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "get wallet failed: {fetched}");
    assert_eq!(
        fetched["address"], first_address,
        "existing wallet address must be preserved after a rejected duplicate create"
    );
}

/// #948 — wallets can only be created on networks the deposit worker polls.
#[tokio::test]
async fn wallet_create_rejects_unsupported_network() {
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "wallet_bad_network").await;

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

/// #1010 — list endpoints carry an ETag and answer a matching
/// If-None-Match with 304 and no body.
#[tokio::test]
async fn transactions_list_supports_etag_revalidation() {
    use tower::ServiceExt;
    let app = app().await;
    let (token, _) = ensure_merchant(&app, "etag").await;

    let (status, _, headers) =
        common::send_with_response_headers(app.clone(), "GET", "/transactions", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get("etag").expect("etag header").to_str().unwrap().to_string();

    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/transactions")
        .header("authorization", format!("Bearer {token}"))
        .header("if-none-match", &etag)
        .body(axum::body::Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
}
