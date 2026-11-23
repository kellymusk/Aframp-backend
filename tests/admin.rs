use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::common::{create_test_admin, create_test_app, login_admin};

#[tokio::test]
async fn admin_wallets_requires_pii_or_financials_role() {
    let app = create_test_app().await;

    // Admin authenticated but without PII/financials role.
    let admin = create_test_admin(&app, "wallets-no-role@example.com", vec![]).await;
    let token = login_admin(&app, &admin).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/admin/wallets")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_wallets_allows_pii_role() {
    let app = create_test_app().await;

    let admin = create_test_admin(&app, "wallets-pii@example.com", vec!["pii".to_string()]).await;
    let token = login_admin(&app, &admin).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/admin/wallets")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
