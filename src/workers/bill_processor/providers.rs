use super::types::{
    AccountInfo, BillPaymentRequest, BillPaymentResponse, PaymentStatus, ProcessingError,
};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value as JsonValue};
use tracing::debug;

// ---------------------------------------------------------------------------
// Provider Trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait BillPaymentProvider: Send + Sync {
    /// Verify account exists and is active
    async fn verify_account(
        &self,
        provider_code: &str,
        account: &str,
        account_type: &str,
    ) -> Result<AccountInfo, ProcessingError>;

    /// Process a bill payment
    async fn process_payment(
        &self,
        request: BillPaymentRequest,
    ) -> Result<BillPaymentResponse, ProcessingError>;

    /// Query payment status
    async fn query_status(&self, reference: &str) -> Result<PaymentStatus, ProcessingError>;
}

// ---------------------------------------------------------------------------
// Provider selection helpers (also available in parent module)
// ---------------------------------------------------------------------------

/// Determine primary provider for a bill type
pub fn get_primary_provider(bill_type: &str) -> &'static str {
    match bill_type.to_lowercase().as_str() {
        "electricity" => "flutterwave",
        "airtime" | "data" => "vtpass",
        "cable_tv" => "flutterwave",
        _ => "vtpass",
    }
}

/// Get backup providers in order of preference
pub fn get_backup_providers(bill_type: &str) -> Vec<&'static str> {
    match bill_type.to_lowercase().as_str() {
        "electricity" => vec!["vtpass", "paystack"],
        "airtime" | "data" => vec!["flutterwave", "paystack"],
        "cable_tv" => vec!["vtpass", "paystack"],
        _ => vec!["vtpass", "flutterwave", "paystack"],
    }
}

// ---------------------------------------------------------------------------
// Flutterwave Adapter
// ---------------------------------------------------------------------------

pub struct FlutterwaveAdapter {
    client: Client,
    api_key: String,
    base_url: String,
}

impl FlutterwaveAdapter {
    pub fn new(api_key: String, base_url: String) -> Result<Self, ProcessingError> {
        Ok(Self {
            client: Client::new(),
            api_key,
            base_url,
        })
    }

    fn auth_header(&self) -> String {
        format!("Bearer {}", self.api_key)
    }
}

#[async_trait]
impl BillPaymentProvider for FlutterwaveAdapter {
    async fn verify_account(
        &self,
        provider_code: &str,
        account: &str,
        account_type: &str,
    ) -> Result<AccountInfo, ProcessingError> {
        let url = format!("{}/v3/bills/{}/validate", self.base_url, provider_code);

        debug!(
            provider = "flutterwave",
            account = account,
            "Verifying account"
        );

        // Build query string manually to avoid depending on `RequestBuilder::query` resolution
        let url_with_query = format!("{}?customer={}&type={}", url, account, account_type);

        let response = self
            .client
            .get(&url_with_query)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ProcessingError::ProviderError {
                provider: "flutterwave".to_string(),
                reason: format!("verification request failed: {}", e),
            })?;

        if !response.status().is_success() {
            return Err(ProcessingError::ProviderError {
                provider: "flutterwave".to_string(),
                reason: format!("verification failed with status: {}", response.status()),
            });
        }

        let data =
            response
                .json::<JsonValue>()
                .await
                .map_err(|e| ProcessingError::ProviderError {
                    provider: "flutterwave".to_string(),
                    reason: format!("failed to parse response: {}", e),
                })?;

        // Extract verification response data
        Ok(AccountInfo {
            account_number: account.to_string(),
            customer_name: data
                .get("data")
                .and_then(|d| d.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("Unknown")
                .to_string(),
            account_type: account_type.to_string(),
            status: "active".to_string(),
            outstanding_balance: data
                .get("data")
                .and_then(|d| d.get("outstanding_balance"))
                .and_then(|b| b.as_f64()),
            additional_info: serde_json::to_string(&data).unwrap_or_default(),
        })
    }

    async fn process_payment(
        &self,
        request: BillPaymentRequest,
    ) -> Result<BillPaymentResponse, ProcessingError> {
        let url = format!(
            "{}/v3/bills/{}/payment",
            self.base_url, request.provider_code
        );

        let payload = json!({
            "customer": request.account_number,
            "amount": request.amount,
            "recurrence": "ONCE",
            "type": request.account_type,
            "reference": request.transaction_id
        });

        debug!(
            provider = "flutterwave",
            bill_type = request.bill_type,
            amount = request.amount,
            "Processing bill payment"
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&payload)
            .send()
            .await
            .map_err(|e| ProcessingError::ProviderError {
                provider: "flutterwave".to_string(),
                reason: format!("payment request failed: {}", e),
            })?;

        let status = response.status();
        if !status.is_success() {
            let error_msg = response.text().await.unwrap_or_default();
            return Err(ProcessingError::ProviderError {
                provider: "flutterwave".to_string(),
                reason: format!("payment failed with status: {} - {}", status, error_msg),
            });
        }

        let data =
            response
                .json::<JsonValue>()
                .await
                .map_err(|e| ProcessingError::ProviderError {
                    provider: "flutterwave".to_string(),
                    reason: format!("failed to parse response: {}", e),
                })?;

        Ok(BillPaymentResponse {
            provider_reference: data
                .get("data")
                .and_then(|d| d.get("tx_ref"))
                .and_then(|r| r.as_str())
                .unwrap_or("unknown")
                .to_string(),
            token: data
                .get("data")
                .and_then(|d| d.get("token"))
                .and_then(|t| t.as_str())
                .map(|s| s.to_string()),
            status: "completed".to_string(),
            message: data
                .get("message")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string()),
        })
    }

    async fn query_status(&self, reference: &str) -> Result<PaymentStatus, ProcessingError> {
        // Implementation would query Flutterwave for status
        // For now, return a placeholder
        Ok(PaymentStatus {
            provider_reference: reference.to_string(),
            status: "pending".to_string(),
            token: None,
            amount: 0,
            message: None,
        })
    }
}

// ---------------------------------------------------------------------------
// VTPass Adapter
// ---------------------------------------------------------------------------

pub struct VTPassAdapter {
    client: Client,
    api_key: String,
    secret_key: String,
    base_url: String,
}

impl VTPassAdapter {
    pub fn new(
        api_key: String,
        secret_key: String,
        base_url: String,
    ) -> Result<Self, ProcessingError> {
        Ok(Self {
            client: Client::new(),
            api_key,
            secret_key,
            base_url,
        })
    }
}

#[async_trait]
impl BillPaymentProvider for VTPassAdapter {
    async fn verify_account(
        &self,
        provider_code: &str,
        account: &str,
        account_type: &str,
    ) -> Result<AccountInfo, ProcessingError> {
        // VTPass primarily validates through API response, not separate verification
        // Instant validation for airtime/data (phone format check)
        if provider_code.to_lowercase() == "mtn"
            || provider_code.to_lowercase() == "airtel"
            || provider_code.to_lowercase() == "glo"
            || provider_code.to_lowercase() == "9mobile"
        {
            // Validate phone format: 080XXXXXXXX
            if account.len() == 11 && account.starts_with('0') {
                return Ok(AccountInfo {
                    account_number: account.to_string(),
                    customer_name: format!("Phone: {}", account),
                    account_type: "prepaid".to_string(),
                    status: "active".to_string(),
                    outstanding_balance: None,
                    additional_info: "{}".to_string(),
                });
            } else {
                return Err(ProcessingError::AccountVerificationFailed {
                    reason: "Invalid phone format".to_string(),
                });
            }
        }

        Ok(AccountInfo {
            account_number: account.to_string(),
            customer_name: "Account".to_string(),
            account_type: account_type.to_string(),
            status: "active".to_string(),
            outstanding_balance: None,
            additional_info: "{}".to_string(),
        })
    }

    async fn process_payment(
        &self,
        request: BillPaymentRequest,
    ) -> Result<BillPaymentResponse, ProcessingError> {
        let url = format!("{}/api/pay", self.base_url);

        let payload = json!({
            "serviceID": request.provider_code,
            "billersCode": request.account_number,
            "variation_code": request.variation_code,
            "amount": request.amount / 100, // Convert to naira
            "phone": request.phone_number,
            "request_id": request.transaction_id
        });

        debug!(
            provider = "vtpass",
            bill_type = request.bill_type,
            amount = request.amount,
            "Processing bill payment"
        );

        let response = self
            .client
            .post(&url)
            .header("api-key", &self.api_key)
            .header("secret-key", &self.secret_key)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ProcessingError::ProviderError {
                provider: "vtpass".to_string(),
                reason: format!("payment request failed: {}", e),
            })?;

        let status = response.status();
        if !status.is_success() {
            let error_msg = response.text().await.unwrap_or_default();
            return Err(ProcessingError::ProviderError {
                provider: "vtpass".to_string(),
                reason: format!("payment failed with status: {} - {}", status, error_msg),
            });
        }

        let data =
            response
                .json::<JsonValue>()
                .await
                .map_err(|e| ProcessingError::ProviderError {
                    provider: "vtpass".to_string(),
                    reason: format!("failed to parse response: {}", e),
                })?;

        Ok(BillPaymentResponse {
            provider_reference: data
                .get("content")
                .and_then(|c| c.get("transactions"))
                .and_then(|t| t.get("transactionId"))
                .and_then(|id| id.as_str())
                .unwrap_or("unknown")
                .to_string(),
            token: data
                .get("content")
                .and_then(|c| c.get("token"))
                .and_then(|t| t.as_str())
                .map(|s| s.to_string()),
            status: data
                .get("content")
                .and_then(|c| c.get("transactions"))
                .and_then(|t| t.get("status"))
                .and_then(|s| s.as_str())
                .unwrap_or("pending")
                .to_string(),
            message: data
                .get("response_description")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string()),
        })
    }

    async fn query_status(&self, reference: &str) -> Result<PaymentStatus, ProcessingError> {
        // Implementation would query VTPass for status
        Ok(PaymentStatus {
            provider_reference: reference.to_string(),
            status: "pending".to_string(),
            token: None,
            amount: 0,
            message: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Paystack Adapter
// ---------------------------------------------------------------------------

pub struct PaystackAdapter {
    client: Client,
    api_key: String,
    base_url: String,
}

impl PaystackAdapter {
    pub fn new(api_key: String, base_url: String) -> Result<Self, ProcessingError> {
        Ok(Self {
            client: Client::new(),
            api_key,
            base_url,
        })
    }

    fn auth_header(&self) -> String {
        format!("Bearer {}", self.api_key)
    }
}

#[async_trait]
impl BillPaymentProvider for PaystackAdapter {
    async fn verify_account(
        &self,
        provider_code: &str,
        account: &str,
        account_type: &str,
    ) -> Result<AccountInfo, ProcessingError> {
        // Paystack has limited verification, basic validation
        Ok(AccountInfo {
            account_number: account.to_string(),
            customer_name: "Account".to_string(),
            account_type: account_type.to_string(),
            status: "active".to_string(),
            outstanding_balance: None,
            additional_info: "{}".to_string(),
        })
    }

    async fn process_payment(
        &self,
        request: BillPaymentRequest,
    ) -> Result<BillPaymentResponse, ProcessingError> {
        let url = format!("{}/transaction/initialize", self.base_url);

        let payload = json!({
            "email": "billing@aframp.app",
            "amount": request.amount, // in kobo
            "metadata": {
                "account": request.account_number,
                "type": request.bill_type,
                "reference": request.transaction_id
            }
        });

        debug!(
            provider = "paystack",
            bill_type = request.bill_type,
            "Processing bill payment"
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&payload)
            .send()
            .await
            .map_err(|e| ProcessingError::ProviderError {
                provider: "paystack".to_string(),
                reason: format!("payment request failed: {}", e),
            })?;

        let status = response.status();
        if !status.is_success() {
            let error_msg = response.text().await.unwrap_or_default();
            return Err(ProcessingError::ProviderError {
                provider: "paystack".to_string(),
                reason: format!("payment failed with status: {} - {}", status, error_msg),
            });
        }

        let data =
            response
                .json::<JsonValue>()
                .await
                .map_err(|e| ProcessingError::ProviderError {
                    provider: "paystack".to_string(),
                    reason: format!("failed to parse response: {}", e),
                })?;

        Ok(BillPaymentResponse {
            provider_reference: data
                .get("data")
                .and_then(|d| d.get("reference"))
                .and_then(|r| r.as_str())
                .unwrap_or("unknown")
                .to_string(),
            token: None, // Paystack doesn't return tokens directly
            status: "processing".to_string(),
            message: data
                .get("message")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string()),
        })
    }

    async fn query_status(&self, reference: &str) -> Result<PaymentStatus, ProcessingError> {
        // Implementation would query Paystack for status
        Ok(PaymentStatus {
            provider_reference: reference.to_string(),
            status: "pending".to_string(),
            token: None,
            amount: 0,
            message: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flutterwave_creation() {
        let result = FlutterwaveAdapter::new(
            "test_key".to_string(),
            "https://api.flutterwave.com".to_string(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_vtpass_creation() {
        let result = VTPassAdapter::new(
            "test_key".to_string(),
            "test_secret".to_string(),
            "https://api.vtpass.com".to_string(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_paystack_creation() {
        let result = PaystackAdapter::new(
            "test_key".to_string(),
            "https://api.paystack.co".to_string(),
        );
        assert!(result.is_ok());
    }
}
