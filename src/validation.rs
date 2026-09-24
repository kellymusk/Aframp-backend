//! Shared field-level input validators for API request bodies.

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
