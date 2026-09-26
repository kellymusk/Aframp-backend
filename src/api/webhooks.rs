use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};

use crate::error::{forbidden, ApiResult, ErrorCode};
use crate::otp::termii::TermiiWebhookEvent;
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

    match serde_json::from_slice::<TermiiWebhookEvent>(&body) {
        Ok(event) => {
            tracing::info!(
                message_id = event.message_id.as_deref().unwrap_or(""),
                status = event.status.as_deref().unwrap_or(""),
                receiver = event.receiver.as_deref().unwrap_or(""),
                channel = event.channel.as_deref().unwrap_or(""),
                "termii webhook event"
            );
        }
        Err(err) => {
            tracing::warn!(error = %err, body = %String::from_utf8_lossy(&body), "termii webhook: unrecognized payload shape");
        }
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
