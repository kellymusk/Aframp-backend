use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tracing::{error, info};

use crate::services::webhook_processor::WebhookProcessor;

pub struct WebhookRetryWorker {
    processor: Arc<WebhookProcessor>,
    interval_secs: u64,
}

impl WebhookRetryWorker {
    pub fn new(processor: Arc<WebhookProcessor>, interval_secs: u64) -> Self {
        Self {
            processor,
            interval_secs,
        }
    }

    pub async fn run(&self) {
        let mut ticker = interval(Duration::from_secs(self.interval_secs));
        info!(
            interval_secs = self.interval_secs,
            "Webhook retry worker started"
        );

        loop {
            ticker.tick().await;

            match self.processor.retry_pending().await {
                Ok(count) => {
                    if count > 0 {
                        info!(processed = count, "Retried pending webhooks");
                    }
                }
                Err(e) => {
                    error!(error = %e, "Failed to retry pending webhooks");
                }
            }
        }
    }
}
