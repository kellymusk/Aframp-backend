use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, forbidden, not_found, ApiResult, ErrorCode};
use crate::otp::termii::TermiiWebhookEvent;
use crate::AppState;

/// Known event types a merchant may subscribe to.
const KNOWN_EVENTS: &[&str] = &["payment.confirmed", "payment.failed", "payment.refunded"];

#[derive(Debug, Deserialize)]
pub struct CreateSubscriptionRequest {
    pub url: String,
    pub events: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SubscriptionResponse {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub url: String,
    pub events: Vec<String>,
    pub active: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

fn validate_url(url: &str) -> ApiResult<()> {
    if !url.starts_with("https://") || url.len() <= "https://".len() {
        return Err(bad_request(
            ErrorCode::InvalidRequest,
            "webhook url must be a valid HTTPS URL",
        ));
    }
    Ok(())
}

fn validate_events(events: &[String]) -> ApiResult<()> {
    if events.is_empty() {
        return Err(bad_request(
            ErrorCode::InvalidRequest,
            "at least one event is required",
        ));
    }
    for event in events {
        if !KNOWN_EVENTS.contains(&event.as_str()) {
            return Err(bad_request(
                ErrorCode::InvalidRequest,
                "unknown webhook event type",
            ));
        }
    }
    Ok(())
}

/// Create a webhook subscription for the authenticated merchant.
pub async fn create_subscription(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateSubscriptionRequest>,
) -> ApiResult<(StatusCode, Json<SubscriptionResponse>)> {
    validate_url(&payload.url)?;
    validate_events(&payload.events)?;

    let secret_hash = state.webhook_secret_hash(&payload.url);

    let row = sqlx::query_as::<_, (Uuid, Uuid, String, Vec<String>, bool, chrono::DateTime<chrono::Utc>)>(
        "INSERT INTO webhook_subscriptions (merchant_id, url, events, secret_hash, active) \
         VALUES ($1, $2, $3, $4, TRUE) \
         RETURNING id, merchant_id, url, events, active, created_at",
    )
    .bind(auth.merchant_id)
    .bind(&payload.url)
    .bind(&payload.events)
    .bind(&secret_hash)
    .fetch_one(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(SubscriptionResponse {
            id: row.0,
            merchant_id: row.1,
            url: row.2,
            events: row.3,
            active: row.4,
            created_at: row.5,
        }),
    ))
}

/// List the authenticated merchant's webhook subscriptions.
pub async fn list_subscriptions(
    State(state): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<Vec<SubscriptionResponse>>> {
    let rows = sqlx::query_as::<_, (Uuid, Uuid, String, Vec<String>, bool, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, merchant_id, url, events, active, created_at \
         FROM webhook_subscriptions WHERE merchant_id = $1 ORDER BY created_at DESC",
    )
    .bind(auth.merchant_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|row| SubscriptionResponse {
                id: row.0,
                merchant_id: row.1,
                url: row.2,
                events: row.3,
                active: row.4,
                created_at: row.5,
            })
            .collect(),
    ))
}

/// Delete a webhook subscription owned by the authenticated merchant.
pub async fn delete_subscription(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let result = sqlx::query(
        "DELETE FROM webhook_subscriptions WHERE id = $1 AND merchant_id = $2",
    )
    .bind(id)
    .bind(auth.merchant_id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(not_found(ErrorCode::NotFound, "subscription not found"));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Termii's delivery-status webhook (SMS sent/delivered/failed/etc. — see
/// https://developers.termii.com/events-and-reports). Register this URL at
/// https://termii.com/account/webhook/config — it's a single account-wide
/// setting, not something sent per-request.
///
/// Always acks fast once the signature checks out, even on a payload shape
/// this version doesn't recognize: a webhook endpoint that 4xx/5xxs on an
/// unexpected-but-genuine event just trains the provider to retry forever.
///
/// Every verified event is also persisted to `webhook_events` so the table is
/// a real audit trail rather than a misleading empty shell. Termii events are
/// account-wide (not merchant-specific), so `merchant_id` is left NULL — see
/// `migrations/0009_webhook_events_audit.sql` for the rationale.
pub async fn termii(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let signature = headers
        .get("x-termii-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !state.otp_provider.verify_webhook_signature(&body, signature) {
        // Logged so a key mismatch (every genuine event rejected) is visible
        // instead of silent 403s — see README "Termii webhook".
        tracing::warn!(
            signature_present = !signature.is_empty(),
            "termii webhook: signature verification failed"
        );
        return Err(forbidden(ErrorCode::Forbidden, "invalid webhook signature"));
    }

    let (event_type, message_id) = match serde_json::from_slice::<TermiiWebhookEvent>(&body) {
        Ok(event) => {
            tracing::info!(
                message_id = event.message_id.as_deref().unwrap_or(""),
                status = event.status.as_deref().unwrap_or(""),
                receiver = event.receiver.as_deref().unwrap_or(""),
                channel = event.channel.as_deref().unwrap_or(""),
                "termii webhook event"
            );
            (
                event.status.clone().unwrap_or_else(|| "unknown".to_string()),
                event.message_id.clone(),
            )
        }
        Err(err) => {
            tracing::warn!(error = %err, body = %String::from_utf8_lossy(&body), "termii webhook: unrecognized payload shape");
            ("unrecognized".to_string(), None)
        }
    };

    // Persist the event for the audit trail. A failure here must not fail the
    // webhook response — the provider would just retry a genuine event.
    if let Err(err) = sqlx::query(
        "INSERT INTO webhook_events (provider, event_type, external_id, payload) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind("termii")
    .bind(&event_type)
    .bind(message_id.as_deref())
    .bind(serde_json::from_slice::<serde_json::Value>(&body).ok())
    .execute(&state.db)
    .await
    {
        tracing::error!(error = %err, "failed to persist termii webhook event");
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Paystack transfer webhook (transfer.success / transfer.failed /
/// transfer.reversed — see https://paystack.com/docs/payments/webhooks).
///
/// Live-mode transfers are genuinely async (they may require OTP
/// finalization), so the withdrawal is left `pending` when the transfer is
/// initiated. Paystack calls this endpoint once the transfer reaches a
/// terminal state, letting us reconcile the withdrawal and refund the
/// merchant's balance on failure/reversal.
///
/// Register this URL in the Paystack dashboard (Settings → API Keys &
/// Webhooks) as `https://<host>/webhooks/paystack`.
///
/// Like the Termii handler, we ack fast once the signature checks out, even
/// for event types this version doesn't act on: a webhook endpoint that
/// 4xx/5xxs on an unexpected-but-genuine event just trains Paystack to retry
/// forever.
pub async fn paystack(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let signature = headers
        .get("x-paystack-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !state
        .payments
        .verify_paystack_webhook_signature(&body, signature)
    {
        return Err(forbidden(ErrorCode::Forbidden, "invalid webhook signature"));
    }

    match serde_json::from_slice::<crate::services::payments::PaystackWebhookEvent>(&body) {
        Ok(event) => {
            if let Err(err) = state.payments.handle_paystack_webhook_event(event).await {
                tracing::error!(error = %err, "paystack webhook: failed to reconcile transfer event");
                return Err(err);
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, body = %String::from_utf8_lossy(&body), "paystack webhook: unrecognized payload shape");
        }
    }

    Ok(StatusCode::NO_CONTENT)
}
