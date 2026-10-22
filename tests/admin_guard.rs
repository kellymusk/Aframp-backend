//! The `/admin/*` JSON routes are guarded only by the `AdminUser` extractor
//! (the `is_admin` JWT claim). These tests pin that guard down across every
//! admin data route, so an accidental change to the extractor — or a new
//! admin route that forgets it — can't silently expose admin data.

mod common;

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;

use common::{ensure_merchant, extract_otp_code, send, state};

/// Every admin route that returns data. `/admin` itself is the static
/// dashboard shell and is deliberately unauthenticated, so it's excluded.
const ADMIN_DATA_ROUTES: &[&str] = &[
    "/admin/overview",
    "/admin/merchants",
    "/admin/users",
    "/admin/wallets",
    "/admin/transactions",
    "/admin/withdrawals",
    "/admin/payment-requests",
];

/// The subset of [`ADMIN_DATA_ROUTES`] that return lists.
const ADMIN_LIST_ROUTES: &[&str] = &[
    "/admin/merchants",
    "/admin/users",
    "/admin/wallets",
    "/admin/transactions",
    "/admin/withdrawals",
    "/admin/payment-requests",
];

async fn app_and_db() -> (axum::Router, PgPool) {
    let state = state().await;
    let db = state.db.clone();
    (aframp::router(state), db)
}

/// Signs up a merchant, promotes its user to admin directly in the database,
/// then logs in again through the real login + OTP flow — the `is_admin`
/// claim is only baked into tokens issued after the flag is set.
async fn admin_token(app: &axum::Router, db: &PgPool) -> String {
    let (_, merchant_id) = ensure_merchant(app, "admin_guard_admin").await;

    let (email, phone): (String, String) = sqlx::query_as(
        "UPDATE users SET is_admin = true
          WHERE id = (SELECT user_id FROM merchants WHERE id = $1::uuid)
          RETURNING email, phone_number",
    )
    .bind(&merchant_id)
    .fetch_one(db)
    .await
    .unwrap();

    let (status, challenge) = send(
        app.clone(),
        "POST",
        "/login",
        None,
        Some(json!({ "email": email, "password": "Password123!" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "admin login failed: {challenge}");

    let code = extract_otp_code(
        &aframp::otp::mock::last_message_for(&phone).expect("login should send an OTP"),
    );
    let (status, verified) = send(
        app.clone(),
        "POST",
        "/verify-otp",
        None,
        Some(json!({ "challenge_id": challenge["challenge_id"], "code": code })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "admin verify-otp failed: {verified}"
    );
    verified["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn merchant_token_is_forbidden_on_every_admin_route() {
    let (app, _) = app_and_db().await;
    let (token, _) = ensure_merchant(&app, "admin_guard_merchant").await;

    for path in ADMIN_DATA_ROUTES {
        let (status, json) = send(app.clone(), "GET", path, Some(&token), None).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "merchant must get 403 on GET {path}: {json}"
        );
        assert_eq!(json["code"], "FORBIDDEN", "GET {path}");
        assert!(
            !json.is_array(),
            "GET {path} must not leak rows to a merchant"
        );
    }
}

#[tokio::test]
async fn unauthenticated_request_is_unauthorized_on_every_admin_route() {
    let (app, _) = app_and_db().await;

    for path in ADMIN_DATA_ROUTES {
        let (status, json) = send(app.clone(), "GET", path, None, None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "anonymous must get 401 on GET {path}: {json}"
        );
        assert_eq!(json["code"], "INVALID_CREDENTIALS", "GET {path}");
    }
}

#[tokio::test]
async fn invalid_token_is_unauthorized_on_every_admin_route() {
    let (app, _) = app_and_db().await;

    for path in ADMIN_DATA_ROUTES {
        let (status, json) = send(app.clone(), "GET", path, Some("not-a-real-jwt"), None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "garbage token must get 401 on GET {path}: {json}"
        );
    }
}

#[tokio::test]
async fn admin_token_can_read_overview() {
    let (app, db) = app_and_db().await;
    let token = admin_token(&app, &db).await;

    let (status, json) = send(app.clone(), "GET", "/admin/overview", Some(&token), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "admin should read /admin/overview: {json}"
    );
    assert!(json.is_object(), "overview should be a JSON object: {json}");
}

#[tokio::test]
async fn admin_token_can_read_every_admin_list_route() {
    let (app, db) = app_and_db().await;
    let token = admin_token(&app, &db).await;

    for path in ADMIN_LIST_ROUTES {
        let (status, json) = send(app.clone(), "GET", path, Some(&token), None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "admin should read GET {path}: {json}"
        );
        assert!(
            json["data"].is_array(),
            "GET {path} should return a page: {json}"
        );
        assert!(
            json.get("next_cursor").is_some(),
            "GET {path} should carry next_cursor: {json}"
        );
    }
}

#[tokio::test]
async fn admin_routes_matrix_covers_every_list_route() {
    // Guards the matrix itself: every list route must also be in the
    // 401/403 sweep, so adding a route to one list but not the other fails.
    for path in ADMIN_LIST_ROUTES {
        assert!(
            ADMIN_DATA_ROUTES.contains(path),
            "{path} is tested for admin access but not for the 401/403 guard"
        );
    }
}

#[tokio::test]
async fn admin_dashboard_sends_security_headers() {
    let app = aframp::router(common::state().await);
    let (status, _, headers) =
        common::send_with_response_headers(app, "GET", "/admin", None, None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let csp = headers["content-security-policy"].to_str().unwrap();
    assert!(csp.starts_with("default-src 'self'"), "{csp}");
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert_eq!(headers["x-frame-options"], "DENY");
    assert_eq!(headers["x-content-type-options"], "nosniff");
}

/// Gives the admin behind `token` the given `admin_role` (read per request).
async fn set_admin_role(app: &axum::Router, db: &PgPool, token: &str, role: Option<&str>) {
    let (_, me) = send(app.clone(), "GET", "/me", Some(token), None).await;
    sqlx::query("UPDATE users SET admin_role = $2 WHERE id = $1::uuid")
        .bind(me["user_id"].as_str().unwrap())
        .bind(role)
        .execute(db)
        .await
        .unwrap();
}

async fn status_of(app: &axum::Router, token: &str, path: &str) -> StatusCode {
    send(app.clone(), "GET", path, Some(token), None).await.0
}

#[tokio::test]
async fn admin_roles_gate_financial_and_personal_data() {
    let (app, db) = app_and_db().await;

    let support = admin_token(&app, &db).await;
    set_admin_role(&app, &db, &support, Some("support")).await;
    assert_eq!(
        status_of(&app, &support, "/admin/users").await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(&app, &support, "/admin/merchants").await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(&app, &support, "/admin/withdrawals").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_of(&app, &support, "/admin/overview").await,
        StatusCode::FORBIDDEN
    );

    let finance = admin_token(&app, &db).await;
    set_admin_role(&app, &db, &finance, Some("finance")).await;
    assert_eq!(
        status_of(&app, &finance, "/admin/withdrawals").await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(&app, &finance, "/admin/overview").await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(&app, &finance, "/admin/users").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_of(&app, &finance, "/admin/merchants").await,
        StatusCode::FORBIDDEN
    );

    // No role recorded (an admin from before roles) keeps full access.
    let legacy = admin_token(&app, &db).await;
    set_admin_role(&app, &db, &legacy, None).await;
    for path in ["/admin/users", "/admin/withdrawals", "/admin/overview"] {
        assert_eq!(
            status_of(&app, &legacy, path).await,
            StatusCode::OK,
            "{path}"
        );
    }
}
