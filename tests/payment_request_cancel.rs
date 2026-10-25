mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, state};

async fn create_wallet(app: &axum::Router, token: &str) {
    let (status, json) = send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "wallet create failed: {json}");
}

fn list_rows(list: &serde_json::Value) -> &Vec<serde_json::Value> {
    list["data"]
        .as_array()
        .or_else(|| list.as_array())
        .expect("list should return data array or page.data")
}

#[tokio::test]
async fn payment_request_cancel_sets_cancelled_at() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 10_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id = created["id"].as_str().unwrap();

    let (status, cancelled) = send(
        app.clone(),
        "DELETE",
        &format!("/payment-requests/{id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "cancel failed: {cancelled}");
    assert_eq!(cancelled["status"], "cancelled");
    assert!(cancelled["cancelled_at"].as_str().is_some());

    // Idempotent reject on second cancel.
    let (status, again) = send(
        app.clone(),
        "DELETE",
        &format!("/payment-requests/{id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "second cancel: {again}");
}

#[tokio::test]
async fn payment_request_cancel_rejects_other_merchants() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token_a, _) = ensure_merchant(&app, "pr_cancel_a").await;
    create_wallet(&app, &token_a).await;
    let (token_b, _) = ensure_merchant(&app, "pr_cancel_b").await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token_a),
        Some(json!({ "amount_stroops": 5_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let id = created["id"].as_str().unwrap();

    let (status, json) = send(
        app.clone(),
        "DELETE",
        &format!("/payment-requests/{id}"),
        Some(&token_b),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "other merchant cancel: {json}"
    );
}

#[tokio::test]
async fn payment_request_list_excludes_cancelled_by_default() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_list_cancel").await;
    create_wallet(&app, &token).await;

    let (status, keep) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 11_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, drop) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 22_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let drop_id = drop["id"].as_str().unwrap();

    let (status, _) = send(
        app.clone(),
        "DELETE",
        &format!("/payment-requests/{drop_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, list) = send(app.clone(), "GET", "/payment-requests", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "list failed: {list}");
    let rows = list_rows(&list);
    assert_eq!(rows.len(), 1, "default list must hide cancelled: {list}");
    assert_eq!(rows[0]["id"], keep["id"]);

    let (status, list_all) = send(
        app.clone(),
        "GET",
        "/payment-requests?include_cancelled=true",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "list all failed: {list_all}");
    let rows_all = list_rows(&list_all);
    assert_eq!(
        rows_all.len(),
        2,
        "include_cancelled should show both: {list_all}"
    );
}

#[tokio::test]
async fn payment_request_hard_delete_job_removes_old_expired_cancelled() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_hard_del").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 7_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let id: uuid::Uuid = created["id"].as_str().unwrap().parse().unwrap();

    let (status, _) = send(
        app.clone(),
        "DELETE",
        &format!("/payment-requests/{id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Age it past the 30-day retention and past expiry.
    sqlx::query(
        "UPDATE payment_requests
            SET cancelled_at = now() - interval '31 days',
                expires_at = now() - interval '31 days'
          WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await
    .unwrap();

    let deleted = aframp::services::payment_requests::hard_delete_expired_cancelled(&state.db)
        .await
        .unwrap();
    assert!(deleted >= 1, "expected at least one hard-delete");

    let remaining: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM payment_requests WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .unwrap();
    assert!(remaining.is_none(), "row should be hard-deleted");
}

async fn create_request(app: &axum::Router, token: &str) -> serde_json::Value {
    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(token),
        Some(json!({ "amount_stroops": 25_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    created
}

#[tokio::test]
async fn payment_request_post_cancel_route_cancels() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_post_cancel").await;
    create_wallet(&app, &token).await;
    let id = create_request(&app, &token).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, cancelled) = send(
        app.clone(),
        "POST",
        &format!("/payment-requests/{id}/cancel"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "cancel failed: {cancelled}");
    assert_eq!(cancelled["status"], "cancelled");
}

#[tokio::test]
async fn payment_request_cancel_unknown_id_returns_404() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel_404").await;
    let unknown = uuid::Uuid::new_v4();

    let (status, _) = send(
        app.clone(),
        "POST",
        &format!("/payment-requests/{unknown}/cancel"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn payment_request_cancel_requires_auth() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel_auth").await;
    create_wallet(&app, &token).await;
    let id = create_request(&app, &token).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, _) = send(
        app.clone(),
        "POST",
        &format!("/payment-requests/{id}/cancel"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn payment_request_cancel_paid_returns_409() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel_paid").await;
    create_wallet(&app, &token).await;
    let created = create_request(&app, &token).await;
    let id: uuid::Uuid = created["id"].as_str().unwrap().parse().unwrap();
    let memo = created["memo"].as_str().unwrap();
    let wallet_id: uuid::Uuid =
        sqlx::query_scalar("SELECT wallet_id FROM payment_requests WHERE id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap();

    let payment_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO payments (id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status)
         VALUES ($1, $2, $3, 'PLACEHOLDER', $4, 25000000, 'XLM', 'stellar', 'confirmed')",
    )
    .bind(payment_id)
    .bind(created["merchant_id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap())
    .bind(wallet_id)
    .bind(format!("test_tx_cancel_{memo}"))
    .execute(&state.db)
    .await
    .unwrap();
    aframp::services::payment_requests::mark_paid(&state.db, id, payment_id)
        .await
        .unwrap();

    let (status, body) = send(
        app.clone(),
        "POST",
        &format!("/payment-requests/{id}/cancel"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a paid request cannot be cancelled: {body}"
    );
    assert_eq!(body["code"], "PAYMENT_REQUEST_NOT_CANCELLABLE");
}

/// `?status=` filters on the status the API reports, so expired and cancelled
/// rows match even though their stored status is still `pending`.
#[tokio::test]
async fn payment_request_list_filters_by_reported_status() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_status_filter").await;
    create_wallet(&app, &token).await;

    let pending = create_request(&app, &token).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let expired = create_request(&app, &token).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let cancelled = create_request(&app, &token).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    sqlx::query(
        "UPDATE payment_requests SET expires_at = now() - interval '1 minute' WHERE id = $1::uuid",
    )
    .bind(&expired)
    .execute(&state.db)
    .await
    .unwrap();
    let (status, _) = send(
        app.clone(),
        "DELETE",
        &format!("/payment-requests/{cancelled}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    for (filter, expected) in [
        ("pending", &pending),
        ("expired", &expired),
        ("cancelled", &cancelled),
    ] {
        let (status, list) = send(
            app.clone(),
            "GET",
            &format!("/payment-requests?status={filter}"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{list}");
        let ids: Vec<&str> = list_rows(&list)
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec![expected.as_str()], "status={filter}");
    }

    let (status, body) = send(
        app.clone(),
        "GET",
        "/payment-requests?status=bogus",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["field"], "status");
}
