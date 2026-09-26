//! #1131 — signup field length coverage via the public validation API.
//! These unit tests do not need a database.

use aframp::validation::{is_valid_email, validate_name, MAX_NAME_LEN};

#[test]
fn name_rejects_over_max_before_any_hashing_would_run() {
    let over = "n".repeat(MAX_NAME_LEN + 1);
    assert!(validate_name(&over).is_err());
    assert!(validate_name(&"n".repeat(MAX_NAME_LEN)).is_ok());
}

#[test]
fn email_rejects_oversized_local_part() {
    let email = format!("{}@example.com", "a".repeat(65));
    assert!(!is_valid_email(&email));
}

#[test]
fn email_accepts_normal_addresses() {
    assert!(is_valid_email("merchant@example.com"));
}

#[test]
fn email_rejects_empty_and_missing_at() {
    assert!(!is_valid_email(""));
    assert!(!is_valid_email("not-an-email"));
}
