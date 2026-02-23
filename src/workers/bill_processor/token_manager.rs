use tracing::{debug, info, warn};

/// Manages bill payment tokens (primarily electricity tokens)
pub struct TokenManager;

impl TokenManager {
    /// Format token for display (electricity token formatting)
    pub fn format_token(token: &str, bill_type: &str) -> String {
        match bill_type.to_lowercase().as_str() {
            "electricity" => Self::format_electricity_token(token),
            "cable_tv" => Self::format_cable_token(token),
            _ => token.to_string(),
        }
    }

    /// Format electricity token (1234-5678-9012-3456 format)
    fn format_electricity_token(token: &str) -> String {
        let token = token.replace("-", "").replace(" ", "");

        if token.len() <= 20 {
            // Already short format, try to format as groups of 4
            let mut formatted = String::new();
            for (i, c) in token.chars().enumerate() {
                if i > 0 && i % 4 == 0 {
                    formatted.push('-');
                }
                formatted.push(c);
            }
            formatted
        } else {
            // Long format, try to format as groups of 6
            let mut formatted = String::new();
            for (i, c) in token.chars().enumerate() {
                if i > 0 && i % 6 == 0 {
                    formatted.push('-');
                }
                formatted.push(c);
            }
            formatted
        }
    }

    /// Format cable TV token
    fn format_cable_token(token: &str) -> String {
        // Mask smart card for security
        if token.len() > 4 {
            format!("{}...{}", &token[..4], &token[token.len() - 4..])
        } else {
            token.to_string()
        }
    }

    /// Validate token format for different bill types
    pub fn validate_token(token: &str, bill_type: &str) -> (bool, String) {
        match bill_type.to_lowercase().as_str() {
            "electricity" => Self::validate_electricity_token(token),
            "airtime" | "data" => (true, "No token validation for airtime/data".to_string()),
            "cable_tv" => Self::validate_cable_token(token),
            _ => (true, "Unknown bill type for token validation".to_string()),
        }
    }

    fn validate_electricity_token(token: &str) -> (bool, String) {
        let clean_token = token.replace("-", "").replace(" ", "");

        if clean_token.is_empty() {
            return (false, "Token is empty".to_string());
        }

        if clean_token.len() < 16 || clean_token.len() > 20 {
            return (
                false,
                format!(
                    "Invalid token length: {} (expected 16-20 characters)",
                    clean_token.len()
                ),
            );
        }

        if !clean_token.chars().all(char::is_numeric) {
            return (
                false,
                "Electricity token must contain only digits".to_string(),
            );
        }

        (true, "Valid electricity token".to_string())
    }

    fn validate_cable_token(token: &str) -> (bool, String) {
        if token.len() < 5 || token.len() > 20 {
            return (
                false,
                format!(
                    "Invalid cable token length: {} (expected 5-20 characters)",
                    token.len()
                ),
            );
        }

        (true, "Valid cable token".to_string())
    }

    /// Store token in database (this would normally call database layer)
    pub async fn store_token(
        transaction_id: &str,
        token: &str,
        bill_type: &str,
    ) -> Result<(), String> {
        debug!(
            transaction_id = transaction_id,
            bill_type = bill_type,
            "Storing bill payment token"
        );

        // Validate token first
        let (valid, msg) = Self::validate_token(token, bill_type);
        if !valid {
            warn!(
                transaction_id = transaction_id,
                reason = msg,
                "Token validation failed"
            );
            return Err(msg);
        }

        info!(
            transaction_id = transaction_id,
            bill_type = bill_type,
            "Token stored successfully"
        );

        // In real implementation, this would call database layer
        Ok(())
    }

    /// Retrieve token from database (this would normally call database layer)
    pub async fn retrieve_token(
        transaction_id: &str,
        bill_type: &str,
    ) -> Result<Option<String>, String> {
        debug!(
            transaction_id = transaction_id,
            bill_type = bill_type,
            "Retrieving bill payment token"
        );

        // In real implementation, this would call database layer
        Ok(None)
    }

    /// Format token for display in notifications
    pub fn format_for_notification(token: Option<&str>, bill_type: &str) -> String {
        match token {
            Some(t) => {
                let formatted = Self::format_token(t, bill_type);
                match bill_type.to_lowercase().as_str() {
                    "electricity" => {
                        format!("**{}**\n\nEnter this token on your meter to load electricity.", formatted)
                    }
                    "airtime" => "Airtime has been delivered to your phone.".to_string(),
                    "data" => "Data bundle has been activated on your line.".to_string(),
                    "cable_tv" => format!(
                        "Your decoder should be active within 5 minutes. Card: {}",
                        formatted
                    ),
                    _ => formatted,
                }
            }
            None => "Payment processed but token not yet available. Please check your account or contact support.".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_electricity_token() {
        let token = "12345678901234567890";
        let formatted = TokenManager::format_electricity_token(token);
        assert!(formatted.contains("-"));
        assert_eq!(formatted.len(), 24); // 20 digits + 4 dashes
    }

    #[test]
    fn test_validate_electricity_token() {
        let (valid, _) = TokenManager::validate_electricity_token("1234-5678-9012-3456");
        assert!(valid);

        let (valid, _) = TokenManager::validate_electricity_token("123");
        assert!(!valid);

        let (valid, _) = TokenManager::validate_electricity_token("abcd-5678-9012-3456");
        assert!(!valid);
    }

    #[test]
    fn test_validate_cable_token() {
        let (valid, _) = TokenManager::validate_cable_token("12345");
        assert!(valid);

        let (valid, _) = TokenManager::validate_cable_token("1234");
        assert!(!valid);
    }

    #[test]
    fn test_format_for_notification() {
        let msg = TokenManager::format_for_notification(Some("1234-5678-9012-3456"), "electricity");
        assert!(msg.contains("meter"));

        let msg = TokenManager::format_for_notification(Some("123"), "airtime");
        assert!(msg.contains("delivered"));

        let msg = TokenManager::format_for_notification(None, "electricity");
        assert!(msg.contains("not yet available"));
    }
}
