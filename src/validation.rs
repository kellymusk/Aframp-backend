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
