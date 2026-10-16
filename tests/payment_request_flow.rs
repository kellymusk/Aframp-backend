mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, send_with_response_headers, state};

async fn create_wallet(app: &axum::Router, token: &str) {
    let (status, json) = send(app.clone(), "POST", "/wallet/create", Some(token), Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "wallet create failed: {json}");
}

#[tokio::test]
async fn payment_request_requires_wallet() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_no_wallet").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 10_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "expected rejection: {json}");
    assert_eq!(json["error"], "create a wallet before generating payment requests");
}

#[tokio::test]
async fn payment_request_create_and_fetch_publicly() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "pr_create").await;
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
    assert_eq!(created["status"], "pending");
    assert_eq!(created["asset"], "XLM", "default asset should be XLM, not cNGN");
    assert_eq!(created["merchant_id"], merchant_id);
    assert!(created["memo"].as_str().unwrap().len() >= 8);
    let sep7 = created["sep7_uri"].as_str().expect("XLM requests should have a sep7_uri");
    assert!(sep7.starts_with("web+stellar:pay?destination="));
    assert!(sep7.contains(&format!("memo={}", created["memo"].as_str().unwrap())));

    // Fetch with NO auth token — this must be publicly readable so a
    // customer's wallet app can look it up before paying.
    let id = created["id"].as_str().unwrap();
    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK, "public fetch failed: {fetched}");
    assert_eq!(fetched["id"], created["id"]);
    assert_eq!(fetched["status"], "pending");
}

#[tokio::test]
async fn payment_request_cngn_has_no_sep7_uri_yet() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cngn").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 10_000_000, "asset": "cNGN" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    assert_eq!(created["asset"], "cNGN");
    assert!(
        created["sep7_uri"].is_null(),
        "cNGN has no configured issuer address yet, so no QR should be generated"
    );
}

#[tokio::test]
async fn payment_request_reports_expired_past_its_expiry() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_expiry").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 5_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id = created["id"].as_str().unwrap();

    // Force it into the past directly — no need to actually wait.
    sqlx::query("UPDATE payment_requests SET expires_at = now() - interval '1 minute' WHERE id = $1::uuid")
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap();

    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["status"], "expired", "past-expiry pending row should report as expired");
}

#[tokio::test]
async fn payment_request_list_is_scoped_to_the_authenticated_merchant() {
    let state = state().await;
    let app = aframp::router(state.clone());

    let (token_a, _) = ensure_merchant(&app, "pr_list_a").await;
    create_wallet(&app, &token_a).await;
    let (token_b, _) = ensure_merchant(&app, "pr_list_b").await;
    create_wallet(&app, &token_b).await;

    for amount in [10_000_000, 20_000_000] {
        let (status, json) = send(
            app.clone(),
            "POST",
            "/payment-requests",
            Some(&token_a),
            Some(json!({ "amount_stroops": amount })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create failed: {json}");
    }
    let (status, json) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token_b),
        Some(json!({ "amount_stroops": 99_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {json}");

    let (status, list_a) = send(app.clone(), "GET", "/payment-requests", Some(&token_a), None).await;
    assert_eq!(status, StatusCode::OK, "list failed: {list_a}");
    let rows = list_a.as_array().unwrap();
    assert_eq!(rows.len(), 2, "merchant A should see only their own two requests");
    // Newest first.
    assert_eq!(rows[0]["amount_stroops"], 20_000_000);
    assert_eq!(rows[1]["amount_stroops"], 10_000_000);
    assert!(
        rows.iter().all(|r| r["sep7_uri"].is_string()),
        "listed XLM requests should each carry a scannable URI"
    );

    let (status, list_b) = send(app.clone(), "GET", "/payment-requests", Some(&token_b), None).await;
    assert_eq!(status, StatusCode::OK);
    let rows_b = list_b.as_array().unwrap();
    assert_eq!(rows_b.len(), 1, "merchant B must not see merchant A's requests");
    assert_eq!(rows_b[0]["amount_stroops"], 99_000_000);
}

#[tokio::test]
async fn payment_request_list_requires_auth() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (status, _) = send(app.clone(), "GET", "/payment-requests", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn payment_request_list_xlm_sep7_uri_is_non_null() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_list_sep7_nonnull").await;
    create_wallet(&app, &token).await;

    // Create two XLM payment requests.
    for amount in [5_000_000i64, 10_000_000i64] {
        let (status, json) = send(
            app.clone(),
            "POST",
            "/payment-requests",
            Some(&token),
            Some(json!({ "amount_stroops": amount })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create failed: {json}");
    }

    let (status, list) = send(app.clone(), "GET", "/payment-requests", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "list failed: {list}");
    let rows = list.as_array().expect("list response should be an array");
    assert!(!rows.is_empty(), "should have at least one payment request in the list");

    for row in rows {
        assert_eq!(row["asset"], "XLM", "test only creates XLM requests");
        let uri = row["sep7_uri"].as_str().unwrap_or_else(|| {
            panic!(
                "sep7_uri must be non-null for XLM request id={} — \
                 if this is null the list query is missing the wallet JOIN",
                row["id"]
            )
        });
        assert!(
            uri.starts_with("web+stellar:pay?destination="),
            "sep7_uri should be a valid SEP-0007 URI: {uri}"
        );
    }
}

#[tokio::test]
async fn deposit_held_pending_until_stellar_confirmation_threshold() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "pr_confirm_depth").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 30_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id: uuid::Uuid = created["id"].as_str().unwrap().parse().unwrap();
    let memo = created["memo"].as_str().unwrap();

    let wallet_id: uuid::Uuid =
        sqlx::query_scalar("SELECT wallet_id FROM payment_requests WHERE id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap();

    // Simulate a detected deposit at the current ledger with a confirmation
    // threshold of 3 ledger closes.
    let detected_ledger: i64 = 1_000;
    let min_confirmations: i64 = 3;

    let fake_payment_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO payments (id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status, ledger_sequence)
         VALUES ($1, $2, $3, 'PLACEHOLDER', $4, 30000000, 'XLM', 'stellar', 'pending', $5)",
    )
    .bind(fake_payment_id)
    .bind(merchant_id.parse::<uuid::Uuid>().unwrap())
    .bind(wallet_id)
    .bind(format!("test_tx_confirm_{memo}"))
    .bind(detected_ledger)
    .execute(&state.db)
    .await
    .unwrap();

    // Before the threshold is reached, the deposit must remain pending and the
    // merchant's available balance must not include it.
    let promoted = aframp::blockchain::worker::promote_confirmed_deposits(
        &state.db,
        detected_ledger + min_confirmations - 1,
        min_confirmations,
    )
    .await
    .unwrap();
    assert_eq!(promoted, 0, "deposit must not be promoted before the threshold");

    let status_after_early: String =
        sqlx::query_scalar("SELECT status FROM payments WHERE id = $1")
            .bind(fake_payment_id)
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(status_after_early, "pending", "deposit should still be pending");

    // Advance the ledger past the threshold — now it should be promoted.
    let promoted = aframp::blockchain::worker::promote_confirmed_deposits(
        &state.db,
        detected_ledger + min_confirmations,
        min_confirmations,
    )
    .await
    .unwrap();
    assert_eq!(promoted, 1, "deposit should be promoted once the threshold is met");

    let status_after: String =
        sqlx::query_scalar("SELECT status FROM payments WHERE id = $1")
            .bind(fake_payment_id)
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(status_after, "confirmed", "deposit should be confirmed after threshold");
}

#[tokio::test]
async fn payment_request_marked_paid_on_memo_correlated_deposit() {
async fn payment_request_list_sep7_uri_matches_get_by_id() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_list_sep7_match").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 15_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id = created["id"].as_str().unwrap();

    // Fetch the single request publicly (as a customer wallet would).
    let (status, by_id) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK, "GET by id failed: {by_id}");
    let sep7_by_id = by_id["sep7_uri"]
        .as_str()
        .expect("GET by id should return a sep7_uri for XLM");

    // Fetch via the authenticated list endpoint.
    let (status, list) = send(app.clone(), "GET", "/payment-requests", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "list failed: {list}");
    let rows = list.as_array().expect("list response should be an array");

    let list_row = rows
        .iter()
        .find(|r| r["id"] == id)
        .expect("the created request should appear in the list");

    let sep7_in_list = list_row["sep7_uri"]
        .as_str()
        .expect("sep7_uri must be non-null in the list for an XLM request");

    assert_eq!(
        sep7_by_id, sep7_in_list,
        "sep7_uri from GET /payment-requests/{{id}} must match the value in the list \
         — a mismatch means the list query uses a different address or parameters"
    );
}


#[tokio::test]
async fn payment_request_marked_paid_on_memo_correlated_deposit() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_paid").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 25_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id: uuid::Uuid = created["id"].as_str().unwrap().parse().unwrap();
    let memo = created["memo"].as_str().unwrap();

    // Simulates exactly what blockchain::worker::process_deposit does once it
    // detects a deposit whose memo matches a pending request — without needing
    // a real signed Stellar transaction (that's a separate, unbuilt capability;
    // see PRD's "Stellar transaction creation" row).
    let wallet_id: uuid::Uuid =
        sqlx::query_scalar("SELECT wallet_id FROM payment_requests WHERE id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap();
    let pending = aframp::services::payment_requests::find_pending_by_wallet_and_memo(&state.db, wallet_id, memo)
        .await
        .unwrap();
    assert!(pending.is_some(), "should find the pending request by wallet_id + memo");

    let fake_payment_id = uuid::Uuid::new_v4();
    // A real payments row is required by the FK — insert one directly, standing
    // in for what payments::record_deposit would have created from a real
    // detected deposit.
    sqlx::query(
        "INSERT INTO payments (id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status)
         VALUES ($1, $2, $3, 'PLACEHOLDER', $4, 25000000, 'XLM', 'stellar', 'confirmed')",
    )
    .bind(fake_payment_id)
    .bind(created["merchant_id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap())
    .bind(wallet_id)
    .bind(format!("test_tx_{memo}"))
    .execute(&state.db)
    .await
    .unwrap();

    aframp::services::payment_requests::mark_paid(&state.db, id, fake_payment_id)
        .await
        .unwrap();

    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["status"], "paid");
}

#[tokio::test]
async fn payment_request_status_endpoint_is_lightweight_and_public() {
async fn payment_after_expiry_is_not_correlated_to_the_request() {
async fn expired_payment_request_is_not_matched_by_deposit_memo() {
async fn payment_request_memos_are_unique_and_fit_a_text_memo() {
async fn amount_stroops_rejects_float_string_and_negative() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_status").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
    let (token, _) = ensure_merchant(&app, "pr_expired_pay").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
    let (token, _) = ensure_merchant(&app, "pr_expired_deposit").await;
    let (token, _) = ensure_merchant(&app, "pr_memo_batch").await;
    create_wallet(&app, &token).await;

    let mut memos = std::collections::HashSet::new();
    for _ in 0..50 {
        let (status, created) = send(
            app.clone(),
            "POST",
            "/payment-requests",
            Some(&token),
            Some(json!({ "amount_stroops": 10_000_000 })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create failed: {created}");
        let memo = created["memo"].as_str().unwrap().to_string();
        assert_eq!(memo.len(), 28, "memo must fit Stellar's 28-byte MEMO_TEXT");
        assert!(memos.insert(memo), "memo repeated within one wallet");
    }
    let (token, _) = ensure_merchant(&app, "pr_amount_types").await;
    create_wallet(&app, &token).await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 25_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id = created["id"].as_str().unwrap();

    // Poll with NO auth token — must be public like the parent endpoint.
    let (status, body) = send(
    let id: uuid::Uuid = created["id"].as_str().unwrap().parse().unwrap();
    let memo = created["memo"].as_str().unwrap();

    // Force the request into the past — the DB row is still 'pending', but it
    // must no longer be eligible for memo correlation.
    sqlx::query("UPDATE payment_requests SET expires_at = now() - interval '1 minute' WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap();

    let wallet_id: uuid::Uuid =
        sqlx::query_scalar("SELECT wallet_id FROM payment_requests WHERE id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap();

    // A deposit arriving after expiry must NOT be correlated to the request.
    let pending = aframp::services::payment_requests::find_pending_by_wallet_and_memo(&state.db, wallet_id, memo)
        .await
        .unwrap();
    assert!(
        pending.is_none(),
        "an expired request must not be returned by find_pending_by_wallet_and_memo"
    );

    // The request must remain untouched: not paid, not partial.
    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["status"], "expired");
    assert_ne!(fetched["status"], "paid");
    assert_ne!(fetched["status"], "partial");

    let db_status: String = sqlx::query_scalar("SELECT status FROM payment_requests WHERE id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(db_status, "pending", "the row itself should be left untouched by the deposit");
}

#[tokio::test]
async fn payment_request_cancel_by_owner_succeeds() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel_ok").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        Some(json!({ "amount_stroops": 2.5 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "float should be 400: {json}");
    assert_eq!(json["code"], "INVALID_PARAMETERS");
    assert_eq!(json["field"], "amount_stroops");
    assert!(
        json["error"].as_str().unwrap().contains("integer"),
        "error should mention integer: {json}"
    );

    let (status, json) = send(
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
        "POST",
        &format!("/payment-requests/{id}/cancel"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "cancel failed: {cancelled}");
    assert_eq!(cancelled["status"], "cancelled");

    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["status"], "cancelled");
}

#[tokio::test]
async fn payment_request_cancel_unknown_id_returns_404() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel_404").await;
    create_wallet(&app, &token).await;

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
        Some(json!({ "amount_stroops": "10000000" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "string should be 400: {json}");
    assert_eq!(json["code"], "INVALID_PARAMETERS");
    assert_eq!(json["field"], "amount_stroops");

    let (status, json) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": -1 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "negative should be 400: {json}");
    assert_eq!(json["code"], "INVALID_AMOUNT");
}

#[tokio::test]
async fn status_endpoint_reports_pending_expired_and_paid() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel_auth").await;
    let (token, _) = ensure_merchant(&app, "pr_status_poll").await;
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
async fn payment_request_cancel_by_other_merchant_is_rejected() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());

    let (token_a, _) = ensure_merchant(&app, "pr_cancel_owner").await;
    create_wallet(&app, &token_a).await;
    let (token_b, _) = ensure_merchant(&app, "pr_cancel_other").await;
    create_wallet(&app, &token_b).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token_a),
        Some(json!({ "amount_stroops": 10_000_000 })),
        Some(json!({ "amount_stroops": 25_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id: uuid::Uuid = created["id"].as_str().unwrap().parse().unwrap();
    let memo = created["memo"].as_str().unwrap();

    sqlx::query("UPDATE payment_requests SET expires_at = now() - interval '1 minute' WHERE id = $1")
        Some(json!({ "amount_stroops": 15_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id = created["id"].as_str().unwrap();

    let (status, _) = send(
        app.clone(),
        "POST",
        &format!("/payment-requests/{id}/cancel"),
        Some(&token_b),
        None,
    )
    .await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
        "another merchant must not be able to cancel this request, got {status}"
    );

    // The request must still be pending for its owner.
    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["status"], "pending");
}

#[tokio::test]
async fn payment_request_cancel_expired_returns_409() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel_expired").await;
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

    let (status, body, headers) = send_with_response_headers(
        app.clone(),
        "GET",
        &format!("/payment-requests/{id}/status"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "public status fetch failed: {body}");
    assert_eq!(body["id"], created["id"]);
    assert_eq!(body["status"], "pending");
    assert!(body["paid_at"].is_null(), "unpaid request should have a null paid_at");
    // The whole point of this endpoint: no bulky fields.
    assert!(body.get("amount_stroops").is_none(), "status payload must omit amount_stroops");
    assert!(body.get("address").is_none(), "status payload must omit address");
    assert!(body.get("sep7_uri").is_none(), "status payload must omit sep7_uri");

    // Unknown ids should 404 rather than leak anything.
    let missing = uuid::Uuid::new_v4();
    let (status, _) = send(
        app.clone(),
        "GET",
        &format!("/payment-requests/{missing}/status"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Now simulate the memo-correlated deposit marking it paid, exactly as the
    // existing paid-flow test does.
    let id_uuid: uuid::Uuid = id.parse().unwrap();
    let memo = created["memo"].as_str().unwrap();
    let wallet_id: uuid::Uuid =
        sqlx::query_scalar("SELECT wallet_id FROM payment_requests WHERE id = $1")
            .bind(id_uuid)
    assert_eq!(status, StatusCode::OK, "status poll failed: {body}");
    assert_eq!(body["status"], "pending");
    assert!(body.get("paid_at").is_none() || body["paid_at"].is_null());
    let cache = headers
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        cache.contains("max-age=5"),
        "expected short Cache-Control TTL, got {cache:?}"
    );

    sqlx::query("UPDATE payment_requests SET expires_at = now() - interval '1 minute' WHERE id = $1::uuid")
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap();

    let (status, _) = send(
        app.clone(),
        "POST",
        &format!("/payment-requests/{id}/cancel"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "an expired request cannot be cancelled");
}

#[tokio::test]
async fn payment_request_cancel_paid_returns_409() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel_paid").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 25_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id: uuid::Uuid = created["id"].as_str().unwrap().parse().unwrap();
    let memo = created["memo"].as_str().unwrap();

    let wallet_id: uuid::Uuid =
        sqlx::query_scalar("SELECT wallet_id FROM payment_requests WHERE id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap();

    let fake_payment_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO payments (id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status)
         VALUES ($1, $2, $3, 'PLACEHOLDER', $4, 25000000, 'XLM', 'stellar', 'confirmed')",
    // This is the lookup blockchain::worker::process_deposit uses to decide
    // which request a memo-matched deposit pays — an expired one must not match.
    let wallet_id: uuid::Uuid =
        sqlx::query_scalar("SELECT wallet_id FROM payment_requests WHERE id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap();
    let pending = aframp::services::payment_requests::find_pending_by_wallet_and_memo(&state.db, wallet_id, memo)
        .await
        .unwrap();
    assert!(pending.is_some(), "should find the pending request by wallet_id + memo");

    let fake_payment_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO payments (id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status)
         VALUES ($1, $2, $3, 'PLACEHOLDER', $4, 25000000, 'XLM', 'stellar', 'confirmed')",
    assert!(pending.is_none(), "an expired request must not be matched to a deposit");

    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["status"], "expired");
    let (status, body) = send(
        app.clone(),
        "GET",
        &format!("/payment-requests/{id}/status"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "expired");

    // Reset expiry and mark paid (same pattern as the full-object paid test).
    sqlx::query("UPDATE payment_requests SET expires_at = now() + interval '15 minutes' WHERE id = $1::uuid")
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap();

    let id_uuid: uuid::Uuid = id.parse().unwrap();
    let wallet_id: uuid::Uuid =
        sqlx::query_scalar("SELECT wallet_id FROM payment_requests WHERE id = $1")
            .bind(id_uuid)
            .fetch_one(&state.db)
            .await
            .unwrap();
    let fake_payment_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO payments (id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status)
         VALUES ($1, $2, $3, 'PLACEHOLDER', $4, 15000000, 'XLM', 'stellar', 'confirmed')",
    )
    .bind(fake_payment_id)
    .bind(created["merchant_id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap())
    .bind(wallet_id)
    .bind(format!("test_tx_status_{memo}"))
    .bind(format!("test_tx_cancel_{memo}"))
    .bind(format!("status_tx_{id}"))
    .execute(&state.db)
    .await
    .unwrap();

    aframp::services::payment_requests::mark_paid(&state.db, id, fake_payment_id)
        .await
        .unwrap();

    let (status, _) = send(
        app.clone(),
        "POST",
        &format!("/payment-requests/{id}/cancel"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "a paid request cannot be cancelled");
    aframp::services::payment_requests::mark_paid(&state.db, id_uuid, fake_payment_id)
        .await
        .unwrap();

    let (status, body) = send(
        app.clone(),
        "GET",
        &format!("/payment-requests/{id}/status"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "status fetch after payment failed: {body}");
    assert_eq!(body["status"], "paid");
    assert!(body["paid_at"].is_string(), "paid request should expose a paid_at timestamp");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "paid");
    assert!(
        body["paid_at"].as_str().is_some(),
        "paid_at should be set when status is paid: {body}"
    );
}

#[tokio::test]
async fn status_endpoint_404_for_unknown_id() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state);
    let missing = uuid::Uuid::new_v4();
    let (status, json) = send(
        app,
        "GET",
        &format!("/payment-requests/{missing}/status"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["code"], "PAYMENT_REQUEST_NOT_FOUND");
}
