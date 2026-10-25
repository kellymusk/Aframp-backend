use axum::extract::State;
use axum::Json;
use serde::Deserialize;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, bad_request_field, conflict, internal, ApiResult, ErrorCode};
use crate::services::merchant_webhooks::{self, MerchantWebhook, RegisterWebhookError};
use crate::AppState;

#[derive(Deserialize)]
pub struct RegisterWebhookRequest {
    pub url: String,
}

/// Register a URL to receive this merchant's outbound events
/// (`payment.confirmed`), signed with `X-Aframp-Signature`.
pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<RegisterWebhookRequest>,
) -> ApiResult<Json<MerchantWebhook>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;

    let webhook = merchant_webhooks::register(&state.db, merchant_id, &req.url)
        .await
        .map_err(|e| match e {
            RegisterWebhookError::InvalidUrl => bad_request_field("url", &e.to_string()),
            RegisterWebhookError::AlreadyRegistered => conflict(ErrorCode::InvalidParameters, &e.to_string()),
            RegisterWebhookError::Database(e) => internal(e),
        })?;
    Ok(Json(webhook))
}

pub async fn list(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Vec<MerchantWebhook>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    let webhooks = merchant_webhooks::list(&state.db, merchant_id)
        .await
        .map_err(internal)?;
    Ok(Json(webhooks))
}
