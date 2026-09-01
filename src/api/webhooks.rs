use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};

use crate::error::{bad_request, internal, not_found, unauthorized, ApiResult};
use crate::payments::paystack::{verify_webhook_signature, PaystackTransferWebhook};
use crate::services::withdrawals::{
    self, PaystackTransferOutcome, PaystackTransferReconciliation, PaystackWebhookError,
};
use crate::AppState;

pub async fn paystack(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let signature = headers
        .get("x-paystack-signature")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| unauthorized("invalid Paystack webhook signature"))?;
    if !verify_webhook_signature(&state.webhook_secret, &body, signature) {
        return Err(unauthorized("invalid Paystack webhook signature"));
    }

    let payload: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| bad_request("invalid Paystack webhook payload"))?;
    let webhook: PaystackTransferWebhook = serde_json::from_value(payload.clone())
        .map_err(|_| bad_request("invalid Paystack webhook payload"))?;

    let outcome = match webhook.event.as_str() {
        "transfer.success" => PaystackTransferOutcome::Completed,
        "transfer.failed" => PaystackTransferOutcome::Failed,
        _ => return Ok(StatusCode::NO_CONTENT),
    };
    let external_id = webhook
        .data
        .external_id()
        .ok_or_else(|| bad_request("Paystack webhook data is missing an event id"))?;
    if webhook.data.reference.is_none() && webhook.data.transfer_code.is_none() {
        return Err(bad_request(
            "Paystack webhook data is missing a transfer reference",
        ));
    }

    let failure_reason = webhook.data.reason.or(webhook.data.message);
    withdrawals::reconcile_paystack_transfer(
        &state.db,
        PaystackTransferReconciliation {
            external_id,
            reference: webhook.data.reference,
            transfer_code: webhook.data.transfer_code,
            outcome,
            failure_reason,
            payload,
        },
    )
    .await
    .map_err(|error| match error {
        PaystackWebhookError::WithdrawalNotFound => {
            not_found("withdrawal referenced by Paystack was not found")
        }
        other => internal(other),
    })?;

    Ok(StatusCode::NO_CONTENT)
}
