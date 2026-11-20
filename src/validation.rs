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
///
/// Note: `+`-extension addresses (e.g. `alice+tag@example.com`) are accepted
/// per RFC 5322. Signup normalises the local part via [`normalize_email`]
/// before the uniqueness check, so `alice@example.com` and
/// `alice+bypass@example.com` resolve to the same account.
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

/// Normalises an email for storage and uniqueness checks.
///
/// Product decision (issue #1423): the local part is lowercased and any
/// `+tag` extension is stripped, and the domain is lowercased. This means
/// `alice@example.com`, `Alice@Example.com`, and `alice+bypass@example.com`
/// all map to the same canonical address, so the same inbox cannot register
/// multiple accounts. Callers should validate with [`is_valid_email`] first;
/// this function does not re-validate and returns the input unchanged if it
/// does not contain an `@`.
pub fn normalize_email(email: &str) -> String {
    let Some((local, domain)) = email.split_once('@') else {
        return email.to_string();
    };

    let local = local.split('+').next().unwrap_or(local);
    format!("{}@{}", local.to_ascii_lowercase(), domain.to_ascii_lowercase())
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
        return Err(
            "phone_number must be a 10-digit Nigerian mobile number (e.g. 0801..., +234801...)",
        );
    }

    Ok(format!("+234{local}"))
}

pub const MAX_NAME_LEN: usize = 100;

/// A client-generated idempotency key is arbitrary but bounded — 255 covers
/// a UUID, a ULID, or a descriptive client request id, while capping how much
/// an unbounded header value could cost to store and index.
pub const MAX_IDEMPOTENCY_KEY_LEN: usize = 255;

/// Well above any real password, but small enough that Argon2 hashing it
/// costs about the same as a normal one — the point is bounding the cost of
/// an oversized input, not accommodating longer passphrases.
///
/// The `RequestBodyLimitLayer` in `main.rs` (MAX_REQUEST_BODY_BYTES) is the primary
/// defence against a huge request body; this is defence in depth against the
/// `password` field specifically, which is the one field whose cost to
/// process scales with its size regardless of body limits.
pub const MAX_PASSWORD_LEN: usize = 1024;

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

/// Validates password complexity requirements:
/// - At least 8 characters (improved from 1Password's minimum)
/// - At least one uppercase letter
/// - At least one digit
/// - At least one special character
///
/// Returns Ok(()) if valid, or Err with a Vec of specific requirement messages.
pub fn validate_password(password: &str) -> Result<(), Vec<&'static str>> {
    let mut errors = Vec::new();

    if password.len() < 8 {
        errors.push("must be at least 8 characters");
    }

    if !password.chars().any(|c| c.is_ascii_uppercase()) {
        errors.push("must contain at least one uppercase letter");
    }

    if !password.chars().any(|c| c.is_ascii_digit()) {
        errors.push("must contain at least one digit");
    }

    if !password.chars().any(|c| !c.is_alphanumeric()) {
        errors.push("must contain at least one special character");
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_complexity_lists_each_missing_requirement() {
        assert!(validate_password("Password123!").is_ok());
        assert_eq!(
            validate_password("password").unwrap_err(),
            vec![
                "must contain at least one uppercase letter",
                "must contain at least one digit",
                "must contain at least one special character",
            ]
        );
        assert!(validate_password("Sh0rt!")
            .unwrap_err()
            .contains(&"must be at least 8 characters"));
    }

    #[test]
    fn is_valid_email_accepts_plus_extension() {
        assert!(is_valid_email("alice@example.com"));
        assert!(is_valid_email("alice+bypass@example.com"));
    }

    #[test]
    fn normalize_email_strips_plus_extension_and_lowercases() {
        assert_eq!(normalize_email("alice@example.com"), "alice@example.com");
        assert_eq!(
            normalize_email("alice+bypass@example.com"),
            "alice@example.com"
        );
        assert_eq!(
            normalize_email("Alice+Spam@Example.COM"),
            "alice@example.com"
        );
        // Same inbox must normalise to the same canonical address.
        assert_eq!(
            normalize_email("alice@example.com"),
            normalize_email("alice+bypass@example.com")
        );
    }

    use proptest::prelude::*;
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
        assert_eq!(
            require_i64(&missing, "amount_stroops").unwrap_err(),
            "is required"
        );
    }

    #[test]
    fn optional_i64_allows_absent_rejects_float() {
        assert_eq!(optional_i64(&json!({}), "expires_in_secs").unwrap(), None);
        assert_eq!(
            optional_i64(&json!({ "expires_in_secs": 60 }), "expires_in_secs").unwrap(),
            Some(60)
        );
        assert_eq!(
            optional_i64(&json!({ "expires_in_secs": 2.5 }), "expires_in_secs").unwrap_err(),
            "must be an integer (int64), not a float"
        );
    }
}
