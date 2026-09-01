mod common;

use aframp::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha512;
use tower::ServiceExt;

use common::{ensure_merchant, send, state};

type HmacSha512 = Hmac<Sha512>;

async fn pending_withdrawal(
    state: &AppState,
    app: &axum::Router,
    seed: &str,
) -> (uuid::Uuid, uuid::Uuid) {
    let (token, merchant_id) = ensure_merchant(app, seed).await;
    let merchant_id = merchant_id.parse().unwrap();
    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1, 'cNGN', 5000000, 0)",
    )
    .bind(merchant_id)
    .execute(&state.db)
    .await
    .unwrap();

    let (status, withdrawal) = send(
        app.clone(),
        "POST",
        "/withdraw",
        Some(&token),
        Some(json!({
            "amount_stroops": 2_000_000,
            "asset": "cNGN",
            "bank_code": "058",
            "account_number": "0123456789"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "withdrawal failed: {withdrawal}");
    (
        withdrawal["id"].as_str().unwrap().parse().unwrap(),
        merchant_id,
    )
}

fn signed_request(payload: &Value, secret: &str, valid_signature: bool) -> Request<Body> {
    let body = serde_json::to_vec(payload).unwrap();
    let signature = if valid_signature {
        let mut mac = HmacSha512::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&body);
        hex::encode(mac.finalize().into_bytes())
    } else {
        "invalid-signature".to_string()
    };

    Request::builder()
        .method("POST")
        .uri("/webhook/paystack")
        .header("content-type", "application/json")
        .header("x-paystack-signature", signature)
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn valid_transfer_success_webhook_completes_withdrawal_and_is_audited() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (withdrawal_id, merchant_id) = pending_withdrawal(&state, &app, "webhook_success").await;
    let event_id = format!("evt_{}", uuid::Uuid::new_v4().simple());
    let payload = json!({
        "event": "transfer.success",
        "data": {
            "id": event_id,
            "reference": withdrawal_id,
            "transfer_code": format!("TRF_{}", uuid::Uuid::new_v4().simple())
        }
    });

    let response = app
        .oneshot(signed_request(&payload, &state.webhook_secret, true))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let status: String = sqlx::query_scalar("SELECT status FROM withdrawals WHERE id = $1")
        .bind(withdrawal_id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(status, "completed");

    let stored_payload: Value = sqlx::query_scalar(
        "SELECT payload FROM webhook_events
          WHERE merchant_id = $1 AND provider = 'paystack' AND external_id = $2",
    )
    .bind(merchant_id)
    .bind(&event_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(stored_payload, payload);
}

#[tokio::test]
async fn valid_transfer_failure_webhook_refunds_only_once() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (withdrawal_id, merchant_id) = pending_withdrawal(&state, &app, "webhook_failure").await;
    let event_id = format!("evt_{}", uuid::Uuid::new_v4().simple());
    let payload = json!({
        "event": "transfer.failed",
        "data": {
            "id": event_id,
            "reference": withdrawal_id,
            "reason": "recipient account rejected"
        }
    });

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(signed_request(&payload, &state.webhook_secret, true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    let (status, failure_reason): (String, Option<String>) =
        sqlx::query_as("SELECT status, failure_reason FROM withdrawals WHERE id = $1")
            .bind(withdrawal_id)
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(
        failure_reason.as_deref(),
        Some("recipient account rejected")
    );

    let balance: i64 = sqlx::query_scalar(
        "SELECT available FROM balances WHERE merchant_id = $1 AND asset = 'cNGN'",
    )
    .bind(merchant_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(
        balance, 5_000_000,
        "duplicate delivery must not refund twice"
    );

    let event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM webhook_events WHERE provider = 'paystack' AND external_id = $1",
    )
    .bind(&event_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(event_count, 1);
}

#[tokio::test]
async fn invalid_paystack_signature_is_rejected_without_reconciliation() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (withdrawal_id, _) = pending_withdrawal(&state, &app, "webhook_bad_signature").await;
    let payload = json!({
        "event": "transfer.success",
        "data": {
            "id": format!("evt_{}", uuid::Uuid::new_v4().simple()),
            "reference": withdrawal_id
        }
    });

    let response = app
        .oneshot(signed_request(&payload, &state.webhook_secret, false))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let status: String = sqlx::query_scalar("SELECT status FROM withdrawals WHERE id = $1")
        .bind(withdrawal_id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(status, "pending");
}
