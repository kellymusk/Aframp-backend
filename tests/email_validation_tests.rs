//! Property-based and regression tests for `validation::is_valid_email`.
//!
//! The custom validator in `src/validation.rs` is hand-rolled, so we fuzz it
//! against a reference implementation (`email_address`) to surface edge cases
//! around consecutive dots, long local parts, and international domain labels.

use email_address::EmailAddress;
use proptest::prelude::*;
use stellar_insights_backend::validation::is_valid_email;

/// Reference validator: parse with the `email_address` crate.
fn reference_is_valid(email: &str) -> bool {
    EmailAddress::is_valid(email)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Arbitrary ASCII strings: whenever the reference validator accepts an
    /// address, our validator must accept it too (no false negatives on the
    /// common accept cases).
    #[test]
    fn accepts_what_reference_accepts(s in "[\x20-\x7e]{0,80}") {
        if reference_is_valid(&s) {
            prop_assert!(
                is_valid_email(&s),
                "reference accepted {:?} but is_valid_email rejected it",
                s
            );
        }
    }

    /// Arbitrary ASCII strings: whenever our validator accepts an address, the
    /// reference must accept it too (no false positives on the common cases).
    #[test]
    fn rejects_what_reference_rejects(s in "[\x20-\x7e]{0,80}") {
        if is_valid_email(&s) {
            prop_assert!(
                reference_is_valid(&s),
                "is_valid_email accepted {:?} but reference rejected it",
                s
            );
        }
    }

    /// Structured generator: build plausible `local@domain` shapes so the
    /// fuzzer spends its budget on near-valid inputs rather than noise.
    #[test]
    fn structured_addresses_agree_with_reference(
        local in "[a-zA-Z0-9._%+\\-]{0,70}",
        domain in "[a-zA-Z0-9.\\-]{0,70}",
    ) {
        let email = format!("{local}@{domain}");
        if reference_is_valid(&email) {
            prop_assert!(
                is_valid_email(&email),
                "reference accepted {:?} but is_valid_email rejected it",
                email
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Regression tests for discrepancies found while fuzzing.
// ---------------------------------------------------------------------------

#[test]
fn rejects_consecutive_dots_in_local_part() {
    assert!(!is_valid_email("john..doe@example.com"));
    assert!(!is_valid_email("john.doe..@example.com"));
    assert!(!is_valid_email("..john@example.com"));
}

#[test]
fn rejects_consecutive_dots_in_domain() {
    assert!(!is_valid_email("john@example..com"));
    assert!(!is_valid_email("john@example.com."));
    assert!(!is_valid_email("john@.example.com"));
}

#[test]
fn rejects_leading_and_trailing_dots_in_local_part() {
    assert!(!is_valid_email(".john@example.com"));
    assert!(!is_valid_email("john.@example.com"));
}

#[test]
fn rejects_local_part_longer_than_64_chars() {
    let local = "a".repeat(65);
    assert!(!is_valid_email(&format!("{local}@example.com")));

    let local = "a".repeat(64);
    assert!(is_valid_email(&format!("{local}@example.com")));
}

#[test]
fn rejects_domain_labels_with_leading_or_trailing_hyphens() {
    assert!(!is_valid_email("john@-example.com"));
    assert!(!is_valid_email("john@example-.com"));
    assert!(!is_valid_email("john@sub.-example.com"));
}

#[test]
fn rejects_domain_without_a_dot() {
    assert!(!is_valid_email("john@localhost"));
}

#[test]
fn rejects_international_domain_labels() {
    // The custom validator is ASCII-only; IDN labels must be rejected rather
    // than silently accepted with a malformed domain.
    assert!(!is_valid_email("john@exämple.com"));
    assert!(!is_valid_email("john@例え.テスト"));
}

#[test]
fn rejects_missing_or_duplicate_at_sign() {
    assert!(!is_valid_email("johnexample.com"));
    assert!(!is_valid_email("john@@example.com"));
    assert!(!is_valid_email("@example.com"));
    assert!(!is_valid_email("john@"));
}

#[test]
fn accepts_common_valid_addresses() {
    assert!(is_valid_email("john.doe@example.com"));
    assert!(is_valid_email("john+tag@example.co.uk"));
    assert!(is_valid_email("a@b.co"));
}
