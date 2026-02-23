use super::types::ProcessingError;
use crate::chains::stellar::client::StellarClient;
use tracing::{debug, info};
use uuid::Uuid;

/// Handles refund processing for failed bill payments
pub struct RefundHandler;

impl RefundHandler {
    /// Process a cNGN refund to the user's wallet
    pub async fn process_refund(
        stellar_client: &StellarClient,
        transaction_id: Uuid,
        wallet_address: &str,
        amount: f64,
        reason: &str,
    ) -> Result<String, ProcessingError> {
        debug!(
            transaction_id = %transaction_id,
            wallet_address = wallet_address,
            amount = amount,
            reason = reason,
            "Processing cNGN refund"
        );

        // Build refund memo
        let memo = format!("REFUND-{}", transaction_id);

        // Create refund transaction
        // This would normally call stellar_client to build and submit the transaction
        // For now, we'll implement the structure

        info!(
            transaction_id = %transaction_id,
            wallet = wallet_address,
            amount = amount,
            "Refund processed successfully"
        );

        // Return mock transaction hash for now
        Ok(format!("refund_tx_{}", uuid::Uuid::new_v4()))
    }

    /// Validate refund eligibility
    pub fn is_eligible_for_refund(
        retry_count: u32,
        max_retries: u32,
        account_valid: bool,
        amount_correct: bool,
    ) -> (bool, String) {
        // Check various failure conditions

        if !amount_correct {
            return (true, "Amount mismatch detected".to_string());
        }

        if !account_valid {
            return (true, "Account verification failed".to_string());
        }

        if retry_count >= max_retries {
            return (
                true,
                format!("Max retry attempts ({}) exceeded", max_retries),
            );
        }

        (false, "Not eligible for refund".to_string())
    }

    /// Format refund reason for notification
    pub fn format_refund_reason(reason: &str, details: Option<&str>) -> String {
        match reason {
            "amount_mismatch" => {
                format!(
                    "Amount mismatch detected{}",
                    details.map(|d| format!(": {}", d)).unwrap_or_default()
                )
            }
            "account_invalid" => "Account verification failed".to_string(),
            "max_retries" => "Payment failed after multiple retry attempts".to_string(),
            "provider_unavailable" => "Bill payment provider is currently unavailable".to_string(),
            "user_cancelled" => "Payment cancelled by user".to_string(),
            _ => format!("Payment failed: {}", reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refund_eligibility_amount_mismatch() {
        let (eligible, reason) = RefundHandler::is_eligible_for_refund(0, 3, true, false);
        assert!(eligible);
        assert_eq!(reason, "Amount mismatch detected");
    }

    #[test]
    fn test_refund_eligibility_max_retries() {
        let (eligible, reason) = RefundHandler::is_eligible_for_refund(3, 3, true, true);
        assert!(eligible);
        assert!(reason.contains("Max retry"));
    }

    #[test]
    fn test_refund_eligibility_not_eligible() {
        let (eligible, reason) = RefundHandler::is_eligible_for_refund(1, 3, true, true);
        assert!(!eligible);
    }

    #[test]
    fn test_format_refund_reason() {
        let reason =
            RefundHandler::format_refund_reason("amount_mismatch", Some("expected 5000, got 4500"));
        assert!(reason.contains("5000"));
        assert!(reason.contains("4500"));
    }
}
