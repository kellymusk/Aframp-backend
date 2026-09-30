//! #1128 — GET /me response must match a single LEFT JOIN of users ⟕ merchants.
//!
//! Production still uses two sequential queries today (`user_by_id` then
//! `merchant_by_user`). This test pins the HTTP contract so a future
//! `users::user_with_merchant` JOIN swap cannot change what clients see.
//! The proposed JOIN itself lives in `docs/proposals/1128-me-join.md` and is
//! exercised here at the SQL layer against the same rows `/me` reads.

mod common;

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use common::{ensure_merchant, send, state};

#[derive(Debug, FromRow)]
struct UserWithMerchantRow {
    user_id: Uuid,
    email: String,
    name: String,
    is_admin: bool,
    created_at: DateTime<Utc>,
    merchant_id: Option<Uuid>,
    merchant_name: Option<String>,
}

/// The single-round-trip query proposed for `users::user_with_merchant`.
async fn join_user_with_merchant(
    db: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<Option<UserWithMerchantRow>, sqlx::Error> {
    sqlx::query_as::<_, UserWithMerchantRow>(
        "SELECT u.id AS user_id,
                u.email,
                u.name,
                u.is_admin,
                u.created_at,
                m.id AS merchant_id,
                m.name AS merchant_name
           FROM users u
           LEFT JOIN merchants m ON m.user_id = u.id
          WHERE u.id = $1
          LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

fn assert_me_matches_join(me: &Value, row: &UserWithMerchantRow) {
    assert_eq!(me["user_id"], row.user_id.to_string());
    assert_eq!(me["email"], row.email);
    assert_eq!(me["name"], row.name);
    assert_eq!(me["is_admin"], row.is_admin);
    assert_eq!(
        me["merchant_id"].as_str().map(|s| s.to_string()),
        row.merchant_id.map(|id| id.to_string())
    );
    assert_eq!(
        me["merchant_name"].as_str().map(|s| s.to_string()),
        row.merchant_name.clone()
    );
    // created_at is RFC3339 either way — just require it is present & equal
    // when parsed.
    let me_created: DateTime<Utc> = me["created_at"]
        .as_str()
        .expect("created_at")
        .parse()
        .expect("rfc3339");
    assert_eq!(me_created, row.created_at);
}

#[tokio::test]
async fn me_response_matches_single_join_query() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "me_join").await;

    let (status, me) = send(app.clone(), "GET", "/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "GET /me failed: {me}");
    assert_eq!(me["merchant_id"], merchant_id);
    assert!(
        me.get("password_hash").is_none(),
        "password hash must never appear in /me"
    );

    let user_id: Uuid = me["user_id"].as_str().unwrap().parse().unwrap();
    let row = join_user_with_merchant(&state.db, user_id)
        .await
        .expect("join query")
        .expect("user row from join");

    assert_me_matches_join(&me, &row);
}

#[tokio::test]
async fn me_join_returns_none_merchant_fields_when_no_merchant_row() {
    // Signup always creates a merchant today, so synthesize the edge case
    // the LEFT JOIN must handle: a user with no merchants row. We delete the
    // merchant after signup and confirm both /me and the JOIN agree.
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "me_join_orphan").await;

    let (status, me_before) = send(app.clone(), "GET", "/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let user_id: Uuid = me_before["user_id"].as_str().unwrap().parse().unwrap();

    // Clear dependent rows then the merchant so the user stands alone.
    sqlx::query("DELETE FROM balances WHERE merchant_id = $1::uuid")
        .bind(&merchant_id)
        .execute(&state.db)
        .await
        .ok();
    sqlx::query("DELETE FROM wallets WHERE merchant_id = $1::uuid")
        .bind(&merchant_id)
        .execute(&state.db)
        .await
        .ok();
    sqlx::query("DELETE FROM merchants WHERE id = $1::uuid")
        .bind(&merchant_id)
        .execute(&state.db)
        .await
        .expect("delete merchant");

    let (status, me) = send(app.clone(), "GET", "/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "GET /me after merchant delete: {me}");
    assert!(me["merchant_id"].is_null(), "merchant_id should be null: {me}");
    assert!(me["merchant_name"].is_null(), "merchant_name should be null: {me}");

    let row = join_user_with_merchant(&state.db, user_id)
        .await
        .expect("join query")
        .expect("user still exists");
    assert!(row.merchant_id.is_none());
    assert!(row.merchant_name.is_none());
    assert_me_matches_join(&me, &row);
}
