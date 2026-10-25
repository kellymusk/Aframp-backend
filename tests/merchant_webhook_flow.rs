mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use serde_json::json;

use aframp::services::merchant_webhooks::{self, RetryPolicy};
use common::{ensure_merchant, send, state};

/// What the test receiver saw: (signature header, event header, body).
type Received = Arc<Mutex<Vec<(String, String, String)>>>;

/// Local HTTP receiver that fails the first `failures` requests with 500 and
/// accepts the rest. Returns its URL and what it received.
async fn receiver(failures: usize) -> (String, Received) {
    let received: Received = Arc::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post({
            let received = received.clone();
            move |headers: HeaderMap, body: String| async move {
                let header = |name: &str| {
                    headers
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string()
                };
                received.lock().unwrap().push((
                    header(merchant_webhooks::SIGNATURE_HEADER),
                    header(merchant_webhooks::EVENT_HEADER),
                    body,
                ));
                if calls.fetch_add(1, Ordering::SeqCst) < failures {
                    StatusCode::INTERNAL_SERVER_ERROR
                } else {
                    StatusCode::OK
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, received)
}

/// Wait until the delivery leaves `pending`, returning (status, attempts).
async fn wait_for_delivery(db: &sqlx::PgPool, id: uuid::Uuid) -> (String, i32) {
    for _ in 0..200 {
        let row: (String, i32) =
            sqlx::query_as("SELECT status, attempts FROM webhook_deliveries WHERE id = $1")
                .bind(id)
                .fetch_one(db)
                .await
                .unwrap();
        if row.0 != "pending" {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("delivery {id} never left pending");
}

fn fast_retries(max_attempts: u32) -> RetryPolicy {
    RetryPolicy {
        max_attempts,
        base_delay: Duration::from_millis(10),
    }
}

#[tokio::test]
async fn register_and_list_webhooks() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "webhook_register").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/webhooks",
        Some(&token),
        Some(json!({ "url": "not a url" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "invalid url should be rejected: {json}");

    let (status, created) = send(
        app.clone(),
        "POST",
        "/webhooks",
        Some(&token),
        Some(json!({ "url": "https://merchant.example.com/aframp" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "register failed: {created}");
    assert_eq!(created["merchant_id"], merchant_id);

    let (status, json) = send(
        app.clone(),
        "POST",
        "/webhooks",
        Some(&token),
        Some(json!({ "url": "https://merchant.example.com/aframp" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "duplicate should conflict: {json}");

    let (status, list) = send(app.clone(), "GET", "/webhooks", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "list failed: {list}");
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["url"], "https://merchant.example.com/aframp");

    let (status, _) = send(app.clone(), "GET", "/webhooks", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dispatch_delivers_signed_payment_confirmed_event() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "webhook_dispatch").await;
    let (url, received) = receiver(0).await;
    let (status, json) = send(app.clone(), "POST", "/webhooks", Some(&token), Some(json!({ "url": url }))).await;
    assert_eq!(status, StatusCode::OK, "register failed: {json}");

    let payload = json!({
        "type": merchant_webhooks::PAYMENT_CONFIRMED,
        "data": { "tx_hash": "abc123", "amount_stroops": 10_000_000 }
    });
    let ids = merchant_webhooks::dispatch(
        &state.db,
        &reqwest::Client::new(),
        state.webhook_secret.as_str(),
        merchant_id.parse().unwrap(),
        merchant_webhooks::PAYMENT_CONFIRMED,
        &payload,
        fast_retries(3),
    )
    .await
    .unwrap();
    assert_eq!(ids.len(), 1);

    assert_eq!(wait_for_delivery(&state.db, ids[0]).await, ("delivered".to_string(), 1));

    let received = received.lock().unwrap();
    let (signature, event, body) = &received[0];
    assert_eq!(event, "payment.confirmed");
    assert_eq!(serde_json::from_str::<serde_json::Value>(body).unwrap(), payload);
    assert_eq!(
        signature,
        &merchant_webhooks::sign(state.webhook_secret.as_str(), body.as_bytes()),
        "signature must be HMAC-SHA256 of the exact body with WEBHOOK_SECRET"
    );
}

#[tokio::test]
async fn dispatch_retries_until_the_receiver_succeeds() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "webhook_retry").await;
    let (url, received) = receiver(2).await;
    send(app.clone(), "POST", "/webhooks", Some(&token), Some(json!({ "url": url }))).await;

    let ids = merchant_webhooks::dispatch(
        &state.db,
        &reqwest::Client::new(),
        state.webhook_secret.as_str(),
        merchant_id.parse().unwrap(),
        merchant_webhooks::PAYMENT_CONFIRMED,
        &json!({ "type": "payment.confirmed" }),
        fast_retries(5),
    )
    .await
    .unwrap();

    assert_eq!(wait_for_delivery(&state.db, ids[0]).await, ("delivered".to_string(), 3));
    assert_eq!(received.lock().unwrap().len(), 3, "two failures, then success");
}

#[tokio::test]
async fn dispatch_marks_delivery_failed_after_max_attempts() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "webhook_exhausted").await;
    let (url, received) = receiver(usize::MAX).await;
    send(app.clone(), "POST", "/webhooks", Some(&token), Some(json!({ "url": url }))).await;

    let ids = merchant_webhooks::dispatch(
        &state.db,
        &reqwest::Client::new(),
        state.webhook_secret.as_str(),
        merchant_id.parse().unwrap(),
        merchant_webhooks::PAYMENT_CONFIRMED,
        &json!({ "type": "payment.confirmed" }),
        fast_retries(3),
    )
    .await
    .unwrap();

    assert_eq!(wait_for_delivery(&state.db, ids[0]).await, ("failed".to_string(), 3));
    assert_eq!(received.lock().unwrap().len(), 3);
    let last_error: Option<String> =
        sqlx::query_scalar("SELECT last_error FROM webhook_deliveries WHERE id = $1")
            .bind(ids[0])
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(last_error.as_deref(), Some("HTTP 500 Internal Server Error"));
}
