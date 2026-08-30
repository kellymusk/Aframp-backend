use crate::chains::stellar::client::StellarClient;
use crate::chains::stellar::errors::StellarResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentEvent {
    pub id: String,
    pub account: String,
    pub from: String,
    pub to: String,
    pub asset_code: Option<String>,
    pub asset_issuer: Option<String>,
    pub amount: String,
    pub created_at: String,
    pub transaction_hash: String,
}

#[async_trait]
pub trait BlockchainListener: Send + Sync {
    async fn start_listening(&self, account: &str) -> StellarResult<mpsc::Receiver<PaymentEvent>>;
    async fn stop_listening(&self);
}

pub struct StellarStreamListener {
    client: StellarClient,
    cursor: Arc<tokio::sync::Mutex<Option<String>>>,
    is_running: Arc<tokio::sync::Mutex<bool>>,
}

impl StellarStreamListener {
    pub fn new(client: StellarClient) -> Self {
        Self {
            client,
            cursor: Arc::new(tokio::sync::Mutex::new(None)),
            is_running: Arc::new(tokio::sync::Mutex::new(false)),
        }
    }

    fn parse_payment_event(&self, data: &str) -> Option<PaymentEvent> {
        match serde_json::from_str::<serde_json::Value>(data) {
            Ok(json) => {
                if json["type"].as_str() != Some("payment") {
                    return None;
                }
                Some(PaymentEvent {
                    id: json["id"].as_str().unwrap_or("").to_string(),
                    account: json["account"].as_str().unwrap_or("").to_string(),
                    from: json["from"].as_str().unwrap_or("").to_string(),
                    to: json["to"].as_str().unwrap_or("").to_string(),
                    asset_code: json["asset_code"].as_str().map(|s| s.to_string()),
                    asset_issuer: json["asset_issuer"].as_str().map(|s| s.to_string()),
                    amount: json["amount"].as_str().unwrap_or("0").to_string(),
                    created_at: json["created_at"].as_str().unwrap_or("").to_string(),
                    transaction_hash: json["transaction_hash"].as_str().unwrap_or("").to_string(),
                })
            }
            Err(e) => {
                warn!("Failed to parse payment event: {}", e);
                None
            }
        }
    }
}

#[async_trait]
impl BlockchainListener for StellarStreamListener {
    async fn start_listening(&self, account: &str) -> StellarResult<mpsc::Receiver<PaymentEvent>> {
        let (tx, rx) = mpsc::channel(100);
        let mut is_running = self.is_running.lock().await;
        *is_running = true;
        drop(is_running);
        
        info!("SSE listener started for account: {}", account);
        Ok(rx)
    }

    async fn stop_listening(&self) {
        let mut is_running = self.is_running.lock().await;
        *is_running = false;
        info!("SSE listener stopped");
    }
}

impl Clone for StellarStreamListener {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            cursor: self.cursor.clone(),
            is_running: self.is_running.clone(),
        }
    }
}
