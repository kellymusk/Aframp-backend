//! Tests for the admin wallets endpoint role guard (issue #1475).
//!
//! `GET /admin/wallets` must require a PII or financials admin role. An
//! authenticated admin without either role must receive `403 Forbidden`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::common::{test_app, TestAdmin};

mod common;

#[tokio::test]
async fn admin_wallets_requires_pii_or_financials_role() {
    let app = test_app().await;

    // Admin authenticated but with no PII/financials role.
    let admin = TestAdmin::new().without_pii().without_financials();
    let token = admin.login(&app).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/admin/wallets")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "admin without PII/financials role must not list wallet addresses"
    );
}

#[tokio::test]
async fn admin_wallets_allows_pii_role() {
    let app = test_app().await;

    let admin = TestAdmin::new().with_pii();
    let token = admin.login(&app).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/admin/wallets")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "admin with PII role must be able to list wallet addresses"
    );
}
