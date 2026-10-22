use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};

use crate::error::{forbidden, internal, ApiResult, ErrorCode};
use crate::otp::termii::TermiiWebhookEvent;
use crate::payments::paystack::{PaystackWebhookEvent, TransferWebhookOutcome};
use crate::AppState;

/// Termii's delivery-status webhook (SMS sent/delivered/failed/etc. — see
/// https://developers.termii.com/events-and-reports). Register this URL at
/// https://termii.com/account/webhook/config — it's a single account-wide
/// setting, not something sent per-request.
///
/// Always acks fast once the signature checks out, even on a payload shape
/// this version doesn't recognize: a webhook endpoint that 4xx/5xxs on an
/// unexpected-but-genuine event just trains the provider to retry forever.
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
            (event.status.unwrap_or_else(|| "unknown".into()), event.message_id)
        }
        Err(err) => {
            tracing::warn!(error = %err, body = %String::from_utf8_lossy(&body), "termii webhook: unrecognized payload shape");
            ("unrecognized".to_string(), None)
        }
    };
    record_termii_event(&state, &body, &event_type, message_id.as_deref()).await;

    Ok(StatusCode::NO_CONTENT)
}

/// Keeps a verified Termii callback in `webhook_events` as an audit record.
/// One message gets a callback per status change, so the key is message id
/// plus status (or the body's hash when there is no id); a retried delivery
/// of the same callback is a no-op. Never fails the request: the provider
/// would only retry a genuine event.
async fn record_termii_event(state: &AppState, body: &[u8], event_type: &str, message_id: Option<&str>) {
    use sha2::{Digest, Sha256};
    let external_id = match message_id {
        Some(id) => format!("{id}:{event_type}"),
        None => format!("sha256:{}", hex::encode(Sha256::digest(body))),
    };
    let payload = serde_json::from_slice::<serde_json::Value>(body)
        .unwrap_or_else(|_| serde_json::json!({ "raw": String::from_utf8_lossy(body) }));
    if let Err(err) = sqlx::query(
        "INSERT INTO webhook_events (provider, external_id, event_type, payload)
         VALUES ('termii', $1, $2, $3)
         ON CONFLICT (provider, external_id) DO NOTHING",
    )
    .bind(&external_id)
    .bind(event_type)
    .bind(payload)
    .execute(&state.db)
    .await
    {
        tracing::error!(error = %err, "failed to record termii webhook event");
    }
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

    if !state.payment_provider.verify_webhook_signature(&body, signature) {
        return Err(forbidden(ErrorCode::Forbidden, "invalid webhook signature"));
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "paystack webhook: body is not JSON");
            return Ok(StatusCode::NO_CONTENT);
        }
    };
    match serde_json::from_value::<PaystackWebhookEvent>(payload.clone()) {
        Ok(event) => {
            let status = match event.outcome() {
                Some(TransferWebhookOutcome::Success) => "completed",
                Some(TransferWebhookOutcome::Failed) => "failed",
                Some(TransferWebhookOutcome::Reversed) => "failed",
                None => return Ok(StatusCode::NO_CONTENT),
            };
            let data = &event.data;
            if data.reference.is_none() && data.transfer_code.is_none() {
                tracing::warn!(event = %event.event, "paystack webhook: transfer event without a reference");
                return Ok(StatusCode::NO_CONTENT);
            }
            let failure_reason = (status == "failed")
                .then(|| data.reason.clone().unwrap_or_else(|| event.event.clone()));
            let external_id = data.id.as_ref().map(|id| match id {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            });
            let updated = crate::services::withdrawals::reconcile_withdrawal_status(
                &state.db,
                data.reference.as_deref(),
                data.transfer_code.as_deref(),
                status,
                failure_reason.as_deref(),
                external_id.as_deref().map(|id| (id, &payload)),
            )
            .await
            .map_err(internal)?;
            if updated.is_none() {
                tracing::warn!(event = %event.event, "paystack webhook: no matching withdrawal");
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "paystack webhook: unrecognized payload shape");
        }
    }

    Ok(StatusCode::NO_CONTENT)
}
