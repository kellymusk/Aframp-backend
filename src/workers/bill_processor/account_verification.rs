use super::providers::BillPaymentProvider;
use super::types::{AccountInfo, ProcessingError, VerificationRequest};
use nuban;
use tracing::{debug, error, info};

/// Account verification logic for different bill types
pub struct AccountVerifier;

impl AccountVerifier {
    /// Verify account based on bill type
    pub async fn verify(
        provider: &dyn BillPaymentProvider,
        request: &VerificationRequest,
    ) -> Result<AccountInfo, ProcessingError> {
        debug!(
            bill_type = request.bill_type,
            account = request.account_number,
            "Verifying account"
        );

        // Central NUBAN validation: if account looks like a Nigerian bank account
        // (10 digits) and provider_code is a 3-digit bank code or the account_type
        // indicates a bank account, validate locally before calling provider.
        if request.account_number.len() == 10
            && (request.provider_code.chars().all(char::is_numeric)
                && request.provider_code.len() == 3
                || request.account_type.to_lowercase().contains("bank"))
        {
            let acct = request.account_number.as_str();
            let bank_code = request.provider_code.as_str();
            // Use the nuban crate API to validate the account
            if nuban::Nuban::new(bank_code, acct).is_err() {
                return Err(ProcessingError::AccountVerificationFailed {
                    reason: "Invalid NUBAN account number".to_string(),
                });
            }
        }

        match request.bill_type.to_lowercase().as_str() {
            "electricity" => Self::verify_electricity(provider, request).await,
            "airtime" => Self::verify_airtime(provider, request).await,
            "data" => Self::verify_data(provider, request).await,
            "cable_tv" => Self::verify_cable_tv(provider, request).await,
            "water" => Self::verify_water(provider, request).await,
            _ => {
                error!(bill_type = request.bill_type, "Unknown bill type");
                Err(ProcessingError::AccountVerificationFailed {
                    reason: format!("Unknown bill type: {}", request.bill_type),
                })
            }
        }
    }

    pub async fn verify_electricity(
        provider: &dyn BillPaymentProvider,
        request: &VerificationRequest,
    ) -> Result<AccountInfo, ProcessingError> {
        debug!(
            provider_code = request.provider_code,
            meter = request.account_number,
            "Verifying electricity meter"
        );

        // Validate meter format
        if request.account_number.len() < 10 || request.account_number.len() > 12 {
            return Err(ProcessingError::AccountVerificationFailed {
                reason: "Invalid meter number format".to_string(),
            });
        }

        if !request.account_number.chars().all(char::is_numeric) {
            return Err(ProcessingError::AccountVerificationFailed {
                reason: "Meter number must contain only digits".to_string(),
            });
        }

        let account_type = if request.account_type.is_empty() {
            "PREPAID"
        } else {
            &request.account_type
        };

        let result = provider
            .verify_account(
                &request.provider_code,
                &request.account_number,
                account_type,
            )
            .await?;

        if result.status.to_lowercase() != "active" {
            return Err(ProcessingError::AccountVerificationFailed {
                reason: format!("Account is not active: {}", result.status),
            });
        }

        info!(
            meter = request.account_number,
            customer = result.customer_name,
            "Electricity meter verified successfully"
        );

        Ok(result)
    }

    async fn verify_airtime(
        _provider: &dyn BillPaymentProvider,
        request: &VerificationRequest,
    ) -> Result<AccountInfo, ProcessingError> {
        let phone = &request.account_number;

        debug!(phone = phone, "Verifying airtime phone number");

        if !Self::is_valid_nigerian_phone(phone) {
            return Err(ProcessingError::AccountVerificationFailed {
                reason: "Invalid Nigerian phone format. Expected 080XXXXXXXX or 07XXXXXXXXX"
                    .to_string(),
            });
        }

        let network = Self::detect_network(phone);
        debug!(phone = phone, network = network, "Detected network");

        Ok(AccountInfo {
            account_number: phone.to_string(),
            customer_name: format!("Phone: {}", phone),
            account_type: "prepaid".to_string(),
            status: "active".to_string(),
            outstanding_balance: None,
            additional_info: format!(r#"{{"network":"{}"}}"#, network),
        })
    }

    async fn verify_data(
        _provider: &dyn BillPaymentProvider,
        request: &VerificationRequest,
    ) -> Result<AccountInfo, ProcessingError> {
        let phone = &request.account_number;

        debug!(phone = phone, "Verifying data bundle phone");

        if !Self::is_valid_nigerian_phone(phone) {
            return Err(ProcessingError::AccountVerificationFailed {
                reason: "Invalid Nigerian phone format".to_string(),
            });
        }

        let network = Self::detect_network(phone);

        Ok(AccountInfo {
            account_number: phone.to_string(),
            customer_name: format!("Phone: {}", phone),
            account_type: "prepaid".to_string(),
            status: "active".to_string(),
            outstanding_balance: None,
            additional_info: format!(r#"{{"network":"{}"}}"#, network),
        })
    }

    async fn verify_cable_tv(
        provider: &dyn BillPaymentProvider,
        request: &VerificationRequest,
    ) -> Result<AccountInfo, ProcessingError> {
        let smart_card = &request.account_number;

        debug!(smart_card = smart_card, "Verifying cable TV smart card");

        if smart_card.len() < 9 || smart_card.len() > 12 {
            return Err(ProcessingError::AccountVerificationFailed {
                reason: "Invalid smart card number format".to_string(),
            });
        }

        if !smart_card.chars().all(char::is_numeric) {
            return Err(ProcessingError::AccountVerificationFailed {
                reason: "Smart card number must contain only digits".to_string(),
            });
        }

        let result = provider
            .verify_account(
                &request.provider_code,
                smart_card,
                if request.account_type.is_empty() {
                    "postpaid"
                } else {
                    &request.account_type
                },
            )
            .await?;

        if result.status.to_lowercase() != "active" {
            return Err(ProcessingError::AccountVerificationFailed {
                reason: format!("Subscription not active: {}", result.status),
            });
        }

        info!(
            smart_card = smart_card,
            customer = result.customer_name,
            "Cable TV subscription verified"
        );

        Ok(result)
    }

    async fn verify_water(
        provider: &dyn BillPaymentProvider,
        request: &VerificationRequest,
    ) -> Result<AccountInfo, ProcessingError> {
        debug!(
            provider_code = request.provider_code,
            meter = request.account_number,
            "Verifying water meter"
        );

        if request.account_number.len() < 10 {
            return Err(ProcessingError::AccountVerificationFailed {
                reason: "Invalid meter number format".to_string(),
            });
        }

        let result = provider
            .verify_account(
                &request.provider_code,
                &request.account_number,
                if request.account_type.is_empty() {
                    "postpaid"
                } else {
                    &request.account_type
                },
            )
            .await?;

        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Helper functions
    // -----------------------------------------------------------------------

    pub fn is_valid_nigerian_phone(phone: &str) -> bool {
        let phone = phone.replace(" ", "").replace("-", "");
        if phone.len() == 11 {
            phone.starts_with('0') && phone.chars().all(char::is_numeric)
        } else if phone.len() == 13 && phone.starts_with("234") {
            phone.chars().all(char::is_numeric)
        } else {
            false
        }
    }

    pub fn detect_network(phone: &str) -> &'static str {
        let phone = phone.trim();
        let normalized = if phone.starts_with("+234") {
            format!("0{}", &phone[4..])
        } else if phone.starts_with("234") {
            format!("0{}", &phone[3..])
        } else {
            phone.to_string()
        };

        let first_digits = normalized.chars().take(3).collect::<String>();
        match first_digits.as_str() {
            "080" | "081" | "090" | "091" => "MTN",
            "070" | "071" => "Airtel",
            "076" | "077" => "Glo",
            "089" => "9Mobile",
            "075" => "Smile",
            "078" => "Spectranet",
            _ => "Unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_nigerian_phone() {
        assert!(AccountVerifier::is_valid_nigerian_phone("08012345678"));
        assert!(AccountVerifier::is_valid_nigerian_phone("09012345678"));
        assert!(AccountVerifier::is_valid_nigerian_phone("2348012345678"));
        assert!(!AccountVerifier::is_valid_nigerian_phone("12345678"));
        assert!(!AccountVerifier::is_valid_nigerian_phone("abc12345678"));
    }

    #[test]
    fn test_network_detection() {
        assert_eq!(AccountVerifier::detect_network("08012345678"), "MTN");
        assert_eq!(AccountVerifier::detect_network("07012345678"), "Airtel");
        assert_eq!(AccountVerifier::detect_network("07612345678"), "Glo");
        assert_eq!(AccountVerifier::detect_network("08912345678"), "9Mobile");
    }
}
