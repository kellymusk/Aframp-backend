mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{ensure_merchant, send, state};

async fn page(app: &axum::Router, token: &str, path: &str, cursor: Option<&str>) -> (Vec<String>, Option<String>) {
    let uri = match cursor {
        Some(c) => format!("{path}?limit=2&cursor={c}"),
        None => format!("{path}?limit=2"),
    };
    let (status, body) = send(app.clone(), "GET", &uri, Some(token), None).await;
    assert_eq!(status, StatusCode::OK, "list failed: {body}");
    let ids = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_string())
        .collect();
    (ids, body["next_cursor"].as_str().map(str::to_string))
}

async fn create_payment_request(app: &axum::Router, token: &str) -> String {
    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(token),
        Some(json!({ "amount_stroops": 10_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    created["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn payment_request_pages_are_stable_under_concurrent_inserts() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "page_pr").await;
    let (status, wallet) = send(app.clone(), "POST", "/wallet/create", Some(&token), Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "wallet create failed: {wallet}");

    let mut original = Vec::new();
    for _ in 0..5 {
        original.push(create_payment_request(&app, &token).await);
    }
    original.reverse(); // newest first, as the API lists them

    let mut seen = Vec::new();
    let (ids, mut cursor) = page(&app, &token, "/payment-requests", None).await;
    seen.extend(ids);
    while let Some(c) = cursor {
        // New requests arriving mid-scroll must not shift or repeat later pages.
        create_payment_request(&app, &token).await;
        let (ids, next) = page(&app, &token, "/payment-requests", Some(&c)).await;
        seen.extend(ids);
        cursor = next;
    }

    assert_eq!(seen, original, "pages must return each original row exactly once, newest first");
}

#[tokio::test]
async fn withdrawal_pages_break_timestamp_ties_by_id() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "page_wd").await;

    // Four withdrawals sharing one created_at: only the id tie-break orders them.
    let rows: Vec<(String,)> = sqlx::query_as(
        "INSERT INTO withdrawals (merchant_id, amount_stroops, asset, status, created_at)
         SELECT $1::uuid, 1000000, 'cNGN', 'completed', '2026-01-01T00:00:00Z'
           FROM generate_series(1, 4)
         RETURNING id::text",
    )
    .bind(&merchant_id)
    .fetch_all(&state.db)
    .await
    .unwrap();
    let mut expected: Vec<String> = rows.into_iter().map(|(id,)| id).collect();
    expected.sort();
    expected.reverse(); // ORDER BY created_at DESC, id DESC

    let mut seen = Vec::new();
    let (ids, mut cursor) = page(&app, &token, "/withdrawals", None).await;
    seen.extend(ids);
    while let Some(c) = cursor {
        let (ids, next) = page(&app, &token, "/withdrawals", Some(&c)).await;
        seen.extend(ids);
        cursor = next;
    }

    assert_eq!(seen, expected);
}

#[tokio::test]
async fn an_invalid_cursor_is_rejected() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "page_bad").await;

    for path in ["/transactions", "/payment-requests", "/withdrawals"] {
        let (status, body): (StatusCode, Value) =
            send(app.clone(), "GET", &format!("{path}?cursor=not-a-cursor"), Some(&token), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {body}");
    }
}
