pub mod transaction_monitor;
pub mod webhook_retry;
pub mod bill_processor {
    pub mod account_verification;
    pub mod payment_executor;
    pub mod providers;
    pub mod refund_handler;
    pub mod token_manager;
    pub mod types;
}

/// Determine if a failed payment is eligible for refund
pub fn is_eligible_for_refund(
    amount_mismatch: i64,
    retry_count: i32,
    account_valid: bool,
    provider_unavailable: bool,
) -> (bool, String) {
    let max_retries = 3;

    if provider_unavailable {
        return (
            true,
            "Bill payment provider is currently unavailable".to_string(),
        );
    }

    if amount_mismatch != 0 {
        return (false, "Amount mismatch detected".to_string());
    }

    if retry_count >= max_retries {
        return (false, "Maximum retry attempts exceeded".to_string());
    }

    if !account_valid {
        return (false, "Account is not valid".to_string());
    }

    (true, "Eligible for refund".to_string())
}
