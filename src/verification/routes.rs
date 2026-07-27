//! Axum router for verification endpoints.

use crate::verification::handlers::*;
use axum::{routing::post, Router};
use std::sync::Arc;

/// Mount at `/auth/verify`.
pub fn verification_router(state: Arc<VerificationState>) -> Router {
    Router::new()
        .route("/email/send", post(send_email_otp))
        .route("/email/confirm", post(confirm_email_otp))
        .route("/phone/send", post(send_phone_otp))
        .route("/phone/confirm", post(confirm_phone_otp))
        .with_state(state)
}
