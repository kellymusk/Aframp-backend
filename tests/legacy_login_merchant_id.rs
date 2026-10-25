//! Regression test for #1265
//!
//! A legacy account (one that pre-dates the mandatory phone-number rollout,
//! stored with `phone_number IS NULL`) that has an associated merchant must
//! receive a non-null `merchant_id` in the `AuthResponse` when it logs in
//! via the password-only (no-OTP) path.
//!
//! The bug: the legacy login branch in `src/api/auth.rs` used the variable
//! `merchant` from a shadowed binding that had already moved into the rate-
//! limiter closure, leaving `merchant_id` as `None` in the `AuthResponse`
//! even though the JWT itself was signed with the correct `merchant_id`.

mod common;

use axum::http::StatusCode;
use serde_json::json;

async fn app() -> (axum::Router, sqlx::PgPool) {
    let state = common::state().await;
    let db = state.db.clone();
    (aframp::router(state), db)
}

/// Seeds a legacy account: a user row with `phone_number = NULL` and a linked
/// merchant row.  Returns `(email, password, merchant_id)`.
async fn seed_legacy_account(db: &sqlx::PgPool) -> (String, String, String) {
    let unique = uuid::Uuid::new_v4();
    let email = format!("legacy+{}@example.com", unique.simple());
    let password = "password123";
    let password_hash = aframp::auth::password::hash(password).expect("hash");
    let name = format!("Legacy User {}", &unique.simple().to_string()[..8]);

    // Insert the user without a phone number (legacy row).
    let user_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash, name, phone_number, is_admin) \
         VALUES ($1, $2, $3, NULL, false) RETURNING id",
    )
    .bind(&email)
    .bind(&password_hash)
    .bind(&name)
    .fetch_one(db)
    .await
    .expect("seed user");

    // Insert the merchant linked to that user.
    let merchant_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO merchants (user_id, name) VALUES ($1, $2) RETURNING id",
    )
    .bind(user_id)
    .bind(&name)
    .fetch_one(db)
    .await
    .expect("seed merchant");

    (email, password.to_string(), merchant_id.to_string())
}

/// A legacy account (phone_number IS NULL) with an existing merchant must get
/// a non-null `merchant_id` field in the login response.
#[tokio::test]
async fn legacy_login_returns_merchant_id() {
    let (app, db) = app().await;
    let (email, password, expected_merchant_id) = seed_legacy_account(&db).await;

    let (status, body) = common::send(
        app,
        "POST",
        "/login",
        None,
        Some(json!({ "email": email, "password": password })),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "login failed: {body}");

    // The token field must be present.
    assert!(
        body["token"].as_str().is_some(),
        "login response must include a token: {body}"
    );

    // Core regression assertion: merchant_id must not be null.
    assert!(
        !body["merchant_id"].is_null(),
        "merchant_id must not be null for a legacy account that has a merchant: {body}"
    );
    assert_eq!(
        body["merchant_id"].as_str().unwrap(),
        expected_merchant_id,
        "merchant_id in response must match the seeded merchant: {body}"
    );
}

/// A legacy account (phone_number IS NULL) without a merchant should get
/// merchant_id = null — that is the legitimate null case and must still work.
#[tokio::test]
async fn legacy_login_merchant_id_null_when_no_merchant() {
    let (app, db) = app().await;

    let unique = uuid::Uuid::new_v4();
    let email = format!("legacy_nomerchant+{}@example.com", unique.simple());
    let password = "password123";
    let password_hash = aframp::auth::password::hash(password).expect("hash");

    sqlx::query(
        "INSERT INTO users (email, password_hash, name, phone_number, is_admin) \
         VALUES ($1, $2, 'No Merchant User', NULL, false)",
    )
    .bind(&email)
    .bind(&password_hash)
    .execute(&db)
    .await
    .expect("seed user without merchant");

    let (status, body) = common::send(
        app,
        "POST",
        "/login",
        None,
        Some(json!({ "email": email, "password": password })),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "login failed: {body}");
    assert!(
        body["merchant_id"].is_null(),
        "merchant_id must be null when no merchant exists for the user: {body}"
    );
}
