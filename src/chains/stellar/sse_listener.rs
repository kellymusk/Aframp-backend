use crate::chains::stellar::client::StellarClient;
use crate::chains::stellar::errors::StellarResult;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Represents a payment event from Horizon SSE stream
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

/// Trait for blockchain payment listeners
#[async_trait]
pub trait BlockchainListener: Send + Sync {
    async fn start_listening(&self, account: &str) -> StellarResult<mpsc::Receiver<PaymentEvent>>;
    async fn stop_listening(&self);
}

/// Stellar Streaming Listener using Horizon SSE
pub struct StellarStreamListener {
    client: StellarClient,
    http_client: Arc<Client>,
    cursor: Arc<tokio::sync::Mutex<Option<String>>>,
    is_running: Arc<tokio::sync::Mutex<bool>>,
}

impl StellarStreamListener {
    pub fn new(client: StellarClient) -> Self {
        Self {
            client,
            http_client: Arc::new(Client::new()),
            cursor: Arc::new(tokio::sync::Mutex::new(None)),
            is_running: Arc::new(tokio::sync::Mutex::new(false)),
        }
    }

    /// Parse Horizon SSE payment response
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

        let client = self.http_client.clone();
        let listener = Arc::new(self.clone());
        let account = account.to_string();

        tokio::spawn(async move {
            let url = format!(
                "https://horizon.stellar.org/accounts/{}/payments?cursor=now",
                account
            );

            loop {
                let is_running = listener.is_running.lock().await;
                if !*is_running {
                    info!("Stopping SSE listener for account {}", account);
                    break;
                }
                drop(is_running);

                match client.get(&url).send().await {
                    Ok(response) => {
                        let mut stream = response.bytes_stream();
                        use futures_util::StreamExt;

                        while let Some(chunk) = stream.next().await {
                            match chunk {
                                Ok(bytes) => {
                                    if let Ok(line) = std::str::from_utf8(&bytes) {
                                        if line.starts_with("data: ") {
                                            let data = &line[6..];
                                            if let Some(event) = listener.parse_payment_event(data) {
                                                if let Err(e) = tx.send(event).await {
                                                    error!("Failed to send payment event: {}", e);
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Error reading SSE stream: {}", e);
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to connect to Horizon SSE: {}", e);
                        // Backoff before retrying
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });

        Ok(rx)
    }

    async fn stop_listening(&self) {
        let mut is_running = self.is_running.lock().await;
        *is_running = false;
        info!("SSE listener stop signal sent");
    }
}

impl Clone for StellarStreamListener {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            http_client: self.http_client.clone(),
            cursor: self.cursor.clone(),
            is_running: self.is_running.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_payment_event() {
        let listener = StellarStreamListener::new(StellarClient::new_public());

        let json_data = r#"{
            "id": "1234567890",
            "type": "payment",
            "account": "GBVP4YKFQW32HMMVHP2HWWPVBKVMSFX5QSCEZCKF4GGJ2YSUKWXL4IUH",
            "from": "GBUQWP3BOUZX34ULNQG23RQ6F4BVXEYMJUCHUOLZMSKSVKNQG77YLVXQ",
            "to": "GBVP4YKFQW32HMMVHP2HWWPVBKVMSFX5QSCEZCKF4GGJ2YSUKWXL4IUH",
            "asset_code": "cNGN",
            "asset_issuer": "GBVVRXLMRYAASFU2HI6FOWZFJC5B5VPVW4XT7NZQA56BTJYDP6KSHJA",
            "amount": "100.0000000",
            "created_at": "2026-08-30T20:50:00Z",
            "transaction_hash": "abc123def456"
        }"#;

        let event = listener.parse_payment_event(json_data);
        assert!(event.is_some());

        let payment = event.unwrap();
        assert_eq!(payment.amount, "100.0000000");
        assert_eq!(payment.asset_code, Some("cNGN".to_string()));
    }

    #[test]
    fn test_ignore_non_payment_events() {
        let listener = StellarStreamListener::new(StellarClient::new_public());

        let json_data = r#"{
            "id": "1234567890",
            "type": "account",
            "account": "GBVP4YKFQW32HMMVHP2HWWPVBKVMSFX5QSCEZCKF4GGJ2YSUKWXL4IUH"
        }"#;

        let event = listener.parse_payment_event(json_data);
        assert!(event.is_none());
    }
}
