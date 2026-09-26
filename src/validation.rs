//! Shared field-level input validators for API request bodies.

use serde_json::Value;

/// Parse a JSON field as a strict `i64` (rejects floats, strings, null, bools).
/// Used so clients get `{ code: INVALID_PARAMETERS, field }` instead of axum's
/// default 422 body when they send `2.5` or `"100"` for an integer amount.
pub fn require_i64(body: &Value, field: &str) -> Result<i64, &'static str> {
    let Some(value) = body.get(field) else {
        return Err("is required");
    };
    parse_i64_value(value)
}

/// Like [`require_i64`], but treats a missing field as `Ok(None)`.
pub fn optional_i64(body: &Value, field: &str) -> Result<Option<i64>, &'static str> {
    match body.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => parse_i64_value(value).map(Some),
    }
}

fn parse_i64_value(value: &Value) -> Result<i64, &'static str> {
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if n.as_u64().is_some() {
                Err("must be a 64-bit signed integer")
            } else {
                // Floating-point (e.g. 2.5) or out-of-range integer.
                Err("must be an integer (int64), not a float")
            }
        }
        Value::String(_) => Err("must be an integer (int64), not a string"),
        Value::Bool(_) => Err("must be an integer (int64), not a boolean"),
        Value::Null => Err("must be an integer (int64)"),
        Value::Array(_) | Value::Object(_) => Err("must be an integer (int64)"),
    }
}

/// Practical RFC 5322 validation: local-part @ domain, with a conservative
/// character set and structural checks (no consecutive/leading/trailing dots,
/// domain must contain at least one dot).
pub fn is_valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };

    if local.is_empty() || local.len() > 64 || domain.is_empty() || domain.len() > 255 {
        return false;
    }

    let local_ok = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-/=?^_`{|}~.".contains(c);
    if !local.chars().all(local_ok)
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
    {
        return false;
    }

    let domain_ok = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '.';
    if !domain.chars().all(domain_ok)
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.starts_with('-')
        || domain.contains("..")
        || !domain.contains('.')
    {
        return false;
    }

    domain
        .split('.')
        .all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-'))
}

/// Nigerian bank codes are a 3-digit string (e.g. "058").
pub fn is_valid_bank_code(code: &str) -> bool {
    code.len() == 3 && code.chars().all(|c| c.is_ascii_digit())
}

/// NUBAN account numbers are exactly 10 digits.
pub fn is_valid_account_number(account_number: &str) -> bool {
    account_number.len() == 10 && account_number.chars().all(|c| c.is_ascii_digit())
}

/// Accepts a Nigerian mobile number in any of the common input shapes
/// (`0801...`, `801...`, `+234801...`, `234801...`, with optional spaces or
/// dashes) and normalizes it to E.164 (`+234801...`) — the shape Termii and
/// every other SMS API expect. Ten digits after the country code, matching
/// the standard Nigerian mobile numbering plan (leading 0 dropped, not part
/// of the subscriber number).
pub fn normalize_ng_phone_number(input: &str) -> Result<String, &'static str> {
    let digits: String = input.chars().filter(|c| c.is_ascii_digit()).collect();

    let local = if let Some(rest) = digits.strip_prefix("234") {
        rest
    } else if let Some(rest) = digits.strip_prefix('0') {
        rest
    } else {
        digits.as_str()
    };

    if local.len() != 10 || !local.chars().all(|c| c.is_ascii_digit()) {
        return Err("phone_number must be a 10-digit Nigerian mobile number (e.g. 0801..., +234801...)");
    }

    Ok(format!("+234{local}"))
}

pub const MAX_NAME_LEN: usize = 100;

/// Trims the name and validates it is non-empty and within the max length.
pub fn validate_name(name: &str) -> Result<String, &'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("name must not be empty");
    }
    if trimmed.chars().count() > MAX_NAME_LEN {
        return Err("name must be at most 100 characters");
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn require_i64_accepts_integers_rejects_float_string() {
        let body = json!({ "amount_stroops": 25_000_000 });
        assert_eq!(require_i64(&body, "amount_stroops").unwrap(), 25_000_000);

        let float = json!({ "amount_stroops": 2.5 });
        assert_eq!(
            require_i64(&float, "amount_stroops").unwrap_err(),
            "must be an integer (int64), not a float"
        );

        let string = json!({ "amount_stroops": "100" });
        assert_eq!(
            require_i64(&string, "amount_stroops").unwrap_err(),
            "must be an integer (int64), not a string"
        );

        let missing = json!({});
        assert_eq!(require_i64(&missing, "amount_stroops").unwrap_err(), "is required");
    }

    #[test]
    fn optional_i64_allows_absent_rejects_float() {
        assert_eq!(optional_i64(&json!({}), "expires_in_secs").unwrap(), None);
        assert_eq!(
            optional_i64(&json!({ "expires_in_secs": 900 }), "expires_in_secs").unwrap(),
            Some(900)
        );
        assert!(optional_i64(&json!({ "expires_in_secs": 1.5 }), "expires_in_secs").is_err());
    }

    #[test]
    fn normalizes_every_common_input_shape_to_e164() {
        for input in ["08011122233", "8011122233", "+2348011122233", "2348011122233", "0801-112-2233"] {
            assert_eq!(normalize_ng_phone_number(input).unwrap(), "+2348011122233", "input: {input}");
        }
    }

    #[test]
    fn rejects_wrong_length_and_non_digit_input() {
        assert!(normalize_ng_phone_number("080111222").is_err()); // too short
        assert!(normalize_ng_phone_number("080111222333").is_err()); // too long
        assert!(normalize_ng_phone_number("not-a-phone").is_err());
        assert!(normalize_ng_phone_number("").is_err());
    }
}
