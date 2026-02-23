pub mod providers;
pub mod types;
pub mod account_verification;
pub mod payment_executor;
pub mod refund_handler;
pub mod token_manager;

// Re-export main types for convenience
pub use types::*;

// -----------------------------------------------------------------------
// Helper functions for provider selection
// -----------------------------------------------------------------------

/// Determine primary provider for a bill type
pub fn get_primary_provider(bill_type: &str) -> &'static str {
    match bill_type.to_lowercase().as_str() {
        "electricity" => "flutterwave",
        "airtime" | "data" => "vtpass",
        "cable_tv" => "flutterwave",
        _ => "vtpass", // Default to VTPass for others
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_selection() {
        assert_eq!(get_primary_provider("electricity"), "flutterwave");
        assert_eq!(get_primary_provider("airtime"), "vtpass");
        assert_eq!(get_primary_provider("cable_tv"), "flutterwave");
    }

    #[test]
    fn test_backup_providers() {
        let backups = get_backup_providers("electricity");
        assert!(backups.contains(&"vtpass"));
        assert!(backups.contains(&"paystack"));
    }
}
