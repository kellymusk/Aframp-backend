use super::providers::BillPaymentProvider;
use super::types::{BillPaymentRequest, BillPaymentResponse, ProcessingError};
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Handles payment execution through bill payment providers
pub struct PaymentExecutor;

impl PaymentExecutor {
    /// Execute a bill payment through the appropriate provider
    pub async fn execute(
        provider: &dyn BillPaymentProvider,
        request: BillPaymentRequest,
    ) -> Result<BillPaymentResponse, ProcessingError> {
        debug!(
            transaction_id = request.transaction_id,
            bill_type = request.bill_type,
            amount = request.amount,
            "Executing bill payment"
        );

        match provider.process_payment(request.clone()).await {
            Ok(response) => {
                info!(
                    transaction_id = request.transaction_id,
                    provider_reference = response.provider_reference,
                    status = response.status,
                    "Bill payment executed successfully"
                );
                Ok(response)
            }
            Err(e) => {
                error!(
                    transaction_id = request.transaction_id,
                    error = %e,
                    "Bill payment execution failed"
                );
                Err(e)
            }
        }
    }

    /// Execute payment with automatic retry logic
    pub async fn execute_with_retry(
        provider: &dyn BillPaymentProvider,
        request: BillPaymentRequest,
        max_retries: u32,
        backoff_seconds: &[u64],
    ) -> Result<BillPaymentResponse, ProcessingError> {
        let mut attempt = 0;

        loop {
            attempt += 1;
            debug!(
                transaction_id = request.transaction_id,
                attempt = attempt,
                max_retries = max_retries,
                "Payment execution attempt"
            );

            match Self::execute(provider, request.clone()).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    if attempt >= max_retries {
                        error!(
                            transaction_id = request.transaction_id,
                            attempts = attempt,
                            "Payment execution failed after max retries"
                        );
                        return Err(ProcessingError::RetryLimitExceeded { attempts: attempt });
                    }

                    // Determine backoff time
                    let backoff_idx = (attempt - 1) as usize;
                    let wait_seconds = backoff_seconds.get(backoff_idx).copied().unwrap_or(300); // Default to 5 minutes

                    warn!(
                        transaction_id = request.transaction_id,
                        attempt = attempt,
                        backoff_seconds = wait_seconds,
                        error = %e,
                        "Payment execution failed, retrying after backoff"
                    );

                    tokio::time::sleep(Duration::from_secs(wait_seconds)).await;
                }
            }
        }
    }

    /// Check payment status and retrieve token if available
    pub async fn check_status_and_retrieve_token(
        provider: &dyn BillPaymentProvider,
        provider_reference: &str,
    ) -> Result<(String, Option<String>), ProcessingError> {
        debug!(
            provider_reference = provider_reference,
            "Checking payment status and retrieving token"
        );

        let status = provider.query_status(provider_reference).await?;

        info!(
            provider_reference = provider_reference,
            status = status.status,
            has_token = status.token.is_some(),
            "Payment status retrieved"
        );

        Ok((status.status, status.token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payment_executor_basic() {
        // Basic test to ensure the module compiles
        // Actual implementation requires mocking provider
    }
}
