//! #1003 — merchant API keys: create (session only), authenticate, list, revoke.
mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, state};

/// #1499 — `POST /api-keys` must record the requesting IP in `api_keys.created_by_ip`.
///
/// Ignored until a maintainer adds the `created_by_ip` column and wires the
/// `src/client_ip.rs` extraction into the create handler; see
/// `docs/proposals/` for the proposal.
#[tokio::test]
#[ignore = "#1499: awaiting maintainer migration + handler wiring for api_keys.created_by_ip"]
async fn api_key_records_creator_ip() {
    let app = aframp::router(state().await);
    let (token, _) = ensure_merchant(&app, "api_keys_creator_ip").await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/api-keys",
        Some(&token),
        Some(json!({ "environment": "test" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let key_id = created["id"].as_str().unwrap().to_string();

    // The test harness forwards a fixed client IP; the created row must echo it.
    let expected_ip = common::TEST_CLIENT_IP;
    let recorded: Option<String> = sqlx::query_scalar(
        "SELECT created_by_ip FROM api_keys WHERE id = $1",
    )
    .bind(&key_id)
    .fetch_one(state().await.pool())
    .await
    .expect("api_keys.created_by_ip must exist");
    assert_eq!(recorded.as_deref(), Some(expected_ip));
}

#[tokio::test]
async fn api_key_lifecycle() {
    let app = aframp::router(state().await);
    let (token, merchant_id) = ensure_merchant(&app, "api_keys").await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/api-keys",
        Some(&token),
        Some(json!({ "environment": "test" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let key = created["secret"].as_str().unwrap().to_string();
    assert!(key.starts_with("sk_test_"));
    let key_id = created["id"].as_str().unwrap().to_string();

    // The key authenticates as the merchant.
    let (status, me) = send(app.clone(), "GET", "/me", Some(&key), None).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["merchant_id"], merchant_id);

    // A key can't mint another key.
    let (status, _) = send(
        app.clone(),
        "POST",
        "/api-keys",
        Some(&key),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Listed without the secret.
    let (status, list) = send(app.clone(), "GET", "/api-keys", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let keys = list.as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert!(keys[0].get("secret").is_none() && keys[0].get("secret_hash").is_none());

    // Revoked keys stop working.
    let (status, _) = send(
        app.clone(),
        "DELETE",
        &format!("/api-keys/{key_id}"),
        Some(&token),
        None,
    )
    .await;
    assert!(status.is_success(), "revoke failed: {status}");
    let (status, _) = send(app.clone(), "GET", "/me", Some(&key), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn malformed_or_unknown_api_key_is_rejected() {
    let app = aframp::router(state().await);
    let unknown = format!("sk_test_{}", "a".repeat(40));
    let (status, _) = send(app.clone(), "GET", "/me", Some(&unknown), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// `GET /api-keys` must only return the calling merchant's keys, never another's.
#[tokio::test]
async fn api_keys_list_is_scoped_to_calling_merchant() {
    let app = aframp::router(state().await);
    let (token_a, _) = ensure_merchant(&app, "api_keys_scope_a").await;
    let (token_b, _) = ensure_merchant(&app, "api_keys_scope_b").await;

    let (status, created_a) = send(
        app.clone(),
        "POST",
        "/api-keys",
        Some(&token_a),
        Some(json!({ "environment": "test" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created_a}");
    let key_a_id = created_a["id"].as_str().unwrap().to_string();

    let (status, list_b) = send(app.clone(), "GET", "/api-keys", Some(&token_b), None).await;
    assert_eq!(status, StatusCode::OK, "{list_b}");
    let keys_b = list_b.as_array().unwrap();
    assert!(
        keys_b
            .iter()
            .all(|k| k["id"].as_str() != Some(key_a_id.as_str())),
        "merchant B's list leaked merchant A's key: {list_b}"
    );
}

/// `GET /api-keys` returns exactly the keys the calling merchant created.
#[tokio::test]
async fn api_keys_list_returns_exactly_own_keys() {
    let app = aframp::router(state().await);
    let (token, _) = ensure_merchant(&app, "api_keys_exact").await;

    let mut created_ids = Vec::new();
    for _ in 0..2 {
        let (status, created) = send(
            app.clone(),
            "POST",
            "/api-keys",
            Some(&token),
            Some(json!({ "environment": "test" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        created_ids.push(created["id"].as_str().unwrap().to_string());
    }

    let (status, list) = send(app.clone(), "GET", "/api-keys", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let keys = list.as_array().unwrap();
    let mut listed_ids: Vec<String> = keys
        .iter()
        .map(|k| k["id"].as_str().unwrap().to_string())
        .collect();
    listed_ids.sort();
    created_ids.sort();
    assert_eq!(listed_ids, created_ids, "unexpected key list: {list}");
}

/// A merchant cannot revoke another merchant's API key.
#[tokio::test]
async fn cannot_revoke_another_merchants_api_key() {
    let app = aframp::router(state().await);
    let (token_a, _) = ensure_merchant(&app, "api_keys_revoke_a").await;
    let (token_b, _) = ensure_merchant(&app, "api_keys_revoke_b").await;

    let (status, created_b) = send(
        app.clone(),
        "POST",
        "/api-keys",
        Some(&token_b),
        Some(json!({ "environment": "test" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created_b}");
    let key_b_id = created_b["id"].as_str().unwrap().to_string();
    let key_b_secret = created_b["secret"].as_str().unwrap().to_string();

    let (status, _) = send(
        app.clone(),
        "DELETE",
        &format!("/api-keys/{key_b_id}"),
        Some(&token_a),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "merchant A must not be able to revoke merchant B's key"
    );

    // B's key still authenticates.
    let (status, _) = send(app.clone(), "GET", "/me", Some(&key_b_secret), None).await;
    assert_eq!(status, StatusCode::OK, "B's key must still work");
}

/// Deleting the account revokes its API keys, as it does its sessions.
#[tokio::test]
async fn api_key_stops_working_after_account_deletion() {
    let app = aframp::router(state().await);
    let (token, _) = ensure_merchant(&app, "api_keys_deleted").await;
    let (status, created) = send(
        app.clone(),
        "POST",
        "/api-keys",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let key = created["secret"].as_str().unwrap().to_string();

    let (status, _) = send(app.clone(), "DELETE", "/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = send(app.clone(), "GET", "/balance", Some(&key), None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a deleted account's key must not authenticate"
    );
}
