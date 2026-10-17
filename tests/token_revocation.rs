//! #1227 — a logged-out JWT is rejected by `authenticate()` afterwards,
//! instead of staying valid for the rest of its 24h lifetime.

mod common;

use axum::http::StatusCode;

use common::{ensure_merchant, send, state};

#[tokio::test]
async fn logged_out_token_is_rejected_on_the_next_request() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state);
    let (token, _) = ensure_merchant(&app, "jti_revoke").await;

    // The token works while the session is live.
    let (status, body) = send(app.clone(), "GET", "/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "pre-logout /me should pass: {body}");

    // Logout clears the cookie *and* revokes the presented jti.
    let (status, body) = send(app.clone(), "POST", "/logout", Some(&token), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "logout failed: {body}");

    // Same token, same signature, still unexpired — but now on the blocklist.
    let (status, body) = send(app.clone(), "GET", "/me", Some(&token), None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "revoked token must be rejected: {body}"
    );

    // A fresh login still works: only the revoked jti is blocked.
    let (token2, _) = ensure_merchant(&app, "jti_revoke_other").await;
    let (status, body) = send(app.clone(), "GET", "/me", Some(&token2), None).await;
    assert_eq!(status, StatusCode::OK, "other sessions unaffected: {body}");
}
