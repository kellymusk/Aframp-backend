mod common;

use axum::http::StatusCode;

use common::{ensure_merchant, send, state};

async fn user_id_for(db: &sqlx::PgPool, merchant_id: &str) -> uuid::Uuid {
    sqlx::query_scalar("SELECT user_id FROM merchants WHERE id = $1::uuid")
        .bind(merchant_id)
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn verify_email_token_is_single_use() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (_, merchant_id) = ensure_merchant(&app, "email_verify").await;
    let user_id = user_id_for(&state.db, &merchant_id).await;

    let token = aframp::services::email_verification::issue_token(&state.db, user_id)
        .await
        .unwrap();

    let (status, body) = send(app.clone(), "GET", &format!("/verify-email?token={token}"), None, None).await;
    assert_eq!(status, StatusCode::OK, "verify failed: {body}");
    assert_eq!(body["email_verified"], true);

    let verified: bool = sqlx::query_scalar("SELECT email_verified FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert!(verified);

    let (status, _) = send(app.clone(), "GET", &format!("/verify-email?token={token}"), None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a consumed token must not verify twice");
}

#[tokio::test]
async fn verify_email_rejects_unknown_token() {
    let state = state().await;
    let app = aframp::router(state);
    let (status, _) = send(app, "GET", "/verify-email?token=not-a-real-token", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn resend_email_verification_requires_auth_and_replaces_token() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "email_resend").await;
    let user_id = user_id_for(&state.db, &merchant_id).await;

    let (status, _) = send(app.clone(), "POST", "/verify-email/resend", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let old = aframp::services::email_verification::issue_token(&state.db, user_id)
        .await
        .unwrap();
    let (status, body) = send(app.clone(), "POST", "/verify-email/resend", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "resend failed: {body}");

    let (status, _) = send(app.clone(), "GET", &format!("/verify-email?token={old}"), None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resend must invalidate the previous token");
}
