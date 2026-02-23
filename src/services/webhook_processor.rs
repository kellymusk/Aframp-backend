use serde_json::Value as JsonValue;
use std::sync::Arc;
use thiserror::Error;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::database::webhook_repository::WebhookRepository;
use crate::payments::factory::PaymentProviderFactory;
use crate::payments::types::ProviderName;
use crate::services::payment_orchestrator::PaymentOrchestrator;

#[derive(Debug, Error)]
pub enum WebhookProcessorError {
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Already processed")]
    AlreadyProcessed,
    #[error("Unknown provider: {0}")]
    UnknownProvider(String),
    #[error("Database error: {0}")]
    DatabaseError(String),
    #[error("Processing error: {0}")]
    ProcessingError(String),
}

pub struct WebhookProcessor {
    webhook_repo: Arc<WebhookRepository>,
    provider_factory: Arc<PaymentProviderFactory>,
    orchestrator: Arc<PaymentOrchestrator>,
}

impl WebhookProcessor {
    pub fn new(
        webhook_repo: Arc<WebhookRepository>,
        provider_factory: Arc<PaymentProviderFactory>,
        orchestrator: Arc<PaymentOrchestrator>,
    ) -> Self {
        Self {
            webhook_repo,
            provider_factory,
            orchestrator,
        }
    }

    pub async fn process_webhook(
        &self,
        provider_name: &str,
        signature: Option<&str>,
        payload: &JsonValue,
    ) -> Result<(), WebhookProcessorError> {
        let provider = self.parse_provider(provider_name)?;
        let signature = signature.ok_or(WebhookProcessorError::InvalidSignature)?;

        let provider_impl = self
            .provider_factory
            .get_provider(provider.clone())
            .map_err(|e| WebhookProcessorError::ProcessingError(e.to_string()))?;

        // Verify signature
        let payload_bytes = serde_json::to_vec(payload)
            .map_err(|e| WebhookProcessorError::ProcessingError(e.to_string()))?;
        let verification = provider_impl
            .verify_webhook(&payload_bytes, signature)
            .map_err(|e| WebhookProcessorError::ProcessingError(e.to_string()))?;

        if !verification.valid {
            error!(provider = %provider_name, "Invalid webhook signature");
            return Err(WebhookProcessorError::InvalidSignature);
        }

        // Parse webhook event
        let event = provider_impl
            .parse_webhook_event(&payload_bytes)
            .map_err(|e| WebhookProcessorError::ProcessingError(e.to_string()))?;

        // Extract event ID for idempotency
        let event_id = self.extract_event_id(&event.payload, provider_name);

        // Check idempotency - log event (will fail if duplicate)
        let webhook_event = self
            .webhook_repo
            .log_event(
                &event_id,
                provider_name,
                &event.event_type,
                payload.clone(),
                Some(signature),
                None,
            )
            .await
            .map_err(|e| WebhookProcessorError::DatabaseError(e.to_string()))?;

        // If already processed, return success
        if webhook_event.status == "completed" {
            info!(event_id = %event_id, "Webhook already processed");
            return Err(WebhookProcessorError::AlreadyProcessed);
        }

        // Process the webhook event
        match self.process_event(&webhook_event, &event).await {
            Ok(_) => {
                self.webhook_repo
                    .mark_processed(webhook_event.id)
                    .await
                    .map_err(|e| WebhookProcessorError::DatabaseError(e.to_string()))?;
                info!(event_id = %event_id, "Webhook processed successfully");
                Ok(())
            }
            Err(e) => {
                warn!(event_id = %event_id, error = %e, "Webhook processing failed");
                self.webhook_repo
                    .record_failure(webhook_event.id, &e.to_string())
                    .await
                    .map_err(|e| WebhookProcessorError::DatabaseError(e.to_string()))?;
                Err(e)
            }
        }
    }

    async fn process_event(
        &self,
        _webhook_event: &crate::database::webhook_repository::WebhookEvent,
        event: &crate::payments::types::WebhookEvent,
    ) -> Result<(), WebhookProcessorError> {
        // Extract transaction reference
        let tx_ref = event
            .transaction_reference
            .as_ref()
            .or(event.provider_reference.as_ref())
            .ok_or_else(|| {
                WebhookProcessorError::ProcessingError("Missing transaction reference".to_string())
            })?;

        // Determine event type and process accordingly
        match event.event_type.as_str() {
            "charge.completed" | "charge.success" => {
                info!(tx_ref = %tx_ref, "Processing payment success webhook");
                self.orchestrator
                    .handle_payment_success(tx_ref)
                    .await
                    .map_err(|e| WebhookProcessorError::ProcessingError(e.to_string()))?;
            }
            "charge.failed" => {
                info!(tx_ref = %tx_ref, "Processing payment failure webhook");
                self.orchestrator
                    .handle_payment_failure(tx_ref, "Payment failed")
                    .await
                    .map_err(|e| WebhookProcessorError::ProcessingError(e.to_string()))?;
            }
            "transfer.completed" | "transfer.success" => {
                info!(tx_ref = %tx_ref, "Processing withdrawal success webhook");
                self.orchestrator
                    .handle_withdrawal_success(tx_ref)
                    .await
                    .map_err(|e| WebhookProcessorError::ProcessingError(e.to_string()))?;
            }
            "transfer.failed" => {
                info!(tx_ref = %tx_ref, "Processing withdrawal failure webhook");
                self.orchestrator
                    .handle_withdrawal_failure(tx_ref, "Withdrawal failed")
                    .await
                    .map_err(|e| WebhookProcessorError::ProcessingError(e.to_string()))?;
            }
            _ => {
                warn!(event_type = %event.event_type, "Unknown webhook event type");
            }
        }

        Ok(())
    }

    fn extract_event_id(&self, payload: &JsonValue, provider: &str) -> String {
        match provider {
            "flutterwave" => payload
                .get("id")
                .and_then(|v| v.as_i64())
                .map(|id| id.to_string())
                .or_else(|| {
                    payload
                        .get("data")
                        .and_then(|d| d.get("id"))
                        .and_then(|v| v.as_i64())
                        .map(|id| id.to_string())
                })
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            "paystack" => payload
                .get("id")
                .and_then(|v| v.as_i64())
                .map(|id| id.to_string())
                .or_else(|| {
                    payload
                        .get("data")
                        .and_then(|d| d.get("id"))
                        .and_then(|v| v.as_i64())
                        .map(|id| id.to_string())
                })
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            _ => Uuid::new_v4().to_string(),
        }
    }

    fn parse_provider(&self, provider: &str) -> Result<ProviderName, WebhookProcessorError> {
        match provider.to_lowercase().as_str() {
            "flutterwave" => Ok(ProviderName::Flutterwave),
            "paystack" => Ok(ProviderName::Paystack),
            "mpesa" => Ok(ProviderName::Mpesa),
            _ => Err(WebhookProcessorError::UnknownProvider(provider.to_string())),
        }
    }

    /// Retry pending webhooks (called by background worker)
    pub async fn retry_pending(&self) -> Result<usize, WebhookProcessorError> {
        let pending = self
            .webhook_repo
            .get_pending_events(50)
            .await
            .map_err(|e| WebhookProcessorError::DatabaseError(e.to_string()))?;

        let mut processed = 0;
        for webhook in pending {
            if webhook.retry_count >= 5 {
                continue;
            }

            let provider_impl = match self.parse_provider(&webhook.provider) {
                Ok(p) => match self.provider_factory.get_provider(p) {
                    Ok(impl_) => impl_,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };

            let payload_bytes = serde_json::to_vec(&webhook.payload).unwrap_or_default();
            let event = match provider_impl.parse_webhook_event(&payload_bytes) {
                Ok(e) => e,
                Err(_) => continue,
            };

            match self.process_event(&webhook, &event).await {
                Ok(_) => {
                    let _ = self.webhook_repo.mark_processed(webhook.id).await;
                    processed += 1;
                }
                Err(e) => {
                    let _ = self
                        .webhook_repo
                        .record_failure(webhook.id, &e.to_string())
                        .await;
                }
            }
        }

        Ok(processed)
    }
}
