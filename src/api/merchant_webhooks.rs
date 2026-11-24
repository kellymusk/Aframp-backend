use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, bad_request_field, conflict, internal, ApiResult, ErrorCode};
use crate::services::merchant_webhooks::{self, MerchantWebhook, RegisterWebhookError};
use crate::AppState;

/// Default page size for `GET /webhooks` when no `limit` is supplied.
const DEFAULT_WEBHOOK_PAGE_LIMIT: usize = 50;
/// Upper bound on `limit` to keep responses bounded.
const MAX_WEBHOOK_PAGE_LIMIT: usize = 100;

#[derive(Deserialize)]
pub struct RegisterWebhookRequest {
    pub url: String,
}

#[derive(Deserialize)]
pub struct ListWebhooksQuery {
    /// Opaque cursor returned as `next_cursor` by a previous page.
    pub cursor: Option<String>,
    /// Maximum number of webhooks to return (defaults to 50, capped at 100).
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct ListWebhooksResponse {
    pub data: Vec<MerchantWebhook>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// Register a URL to receive this merchant's outbound events
/// (`payment.confirmed`), signed with `X-Aframp-Signature`.
pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<RegisterWebhookRequest>,
) -> ApiResult<Json<MerchantWebhook>> {
    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })?;

    let webhook = merchant_webhooks::register(
        &state.db,
        merchant_id,
        &req.url,
        state.webhook_allow_private_targets,
    )
    .await
    .map_err(|e| match e {
        RegisterWebhookError::InvalidUrl | RegisterWebhookError::PrivateTarget => {
            bad_request_field("url", &e.to_string())
        }
        RegisterWebhookError::AlreadyRegistered => {
            conflict(ErrorCode::InvalidParameters, &e.to_string())
        }
        RegisterWebhookError::Database(e) => internal(e),
    })?;
    Ok(Json(webhook))
}

/// List this merchant's registered webhooks with cursor-based pagination.
pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<ListWebhooksQuery>,
) -> ApiResult<Json<ListWebhooksResponse>> {
    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })?;

    let limit = query
        .limit
        .unwrap_or(DEFAULT_WEBHOOK_PAGE_LIMIT)
        .clamp(1, MAX_WEBHOOK_PAGE_LIMIT);

    let mut webhooks = merchant_webhooks::list(&state.db, merchant_id)
        .await
        .map_err(internal)?;

    // Apply cursor: skip everything up to and including the cursor id.
    if let Some(cursor) = query.cursor.as_deref() {
        if let Some(pos) = webhooks.iter().position(|w| w.id == cursor) {
            webhooks.drain(..=pos);
        }
    }

    let has_more = webhooks.len() > limit;
    webhooks.truncate(limit);
    let next_cursor = if has_more {
        webhooks.last().map(|w| w.id.clone())
    } else {
        None
    };

    Ok(Json(ListWebhooksResponse {
        data: webhooks,
        next_cursor,
        has_more,
    }))
}
