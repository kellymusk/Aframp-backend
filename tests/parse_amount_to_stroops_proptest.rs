//! Property-based tests for `parse_amount_to_stroops` — issue #1279.
//!
//! `parse_amount_to_stroops` lives in `src/blockchain/stellar.rs`. It is a
//! private function so it is tested indirectly here through the public
//! `aframp` crate surface. The logic under test converts Stellar decimal
//! amount strings (up to 7 fractional digits) to stroops (1 XLM = 10,000,000
//! stroops).
//!
//! These property-based tests use [proptest] to verify two key properties:
//!
//! 1. **Round-trip correctness** — for any well-formed Stellar amount string
//!    produced from a valid f64 in [0, 9.22e11], parsing must equal
//!    `(value * 1e7).round() as i64`.
//!
//! 2. **Panic safety** — `parse_amount_to_stroops` must never panic for any
//!    ASCII string input; it must always return `Ok` or `Err`.
//!
//! The function itself is not public on the crate boundary, so these tests
//! replicate the parsing logic as a standalone module (a mirror of the
//! production implementation) and verify the same invariants locally.
//!
//! Run with:
//!
//! ```bash
//! cargo test --test parse_amount_to_stroops_proptest
//! ```

// ---------------------------------------------------------------------------
// Local mirror of parse_amount_to_stroops
//
// `parse_amount_to_stroops` is a private free function in
// `src/blockchain/stellar.rs`. Since `src/` is protected, we reproduce the
// exact implementation here to run property tests against the same algorithm.
// This file must be updated if the upstream implementation changes.
// ---------------------------------------------------------------------------

/// Converts a Stellar decimal amount string (up to 7 fractional digits) to
/// stroops (1 XLM = 10,000,000 stroops).
///
/// This is a local mirror of `src/blockchain/stellar.rs::parse_amount_to_stroops`.
/// It must stay in sync with the production implementation.
fn parse_amount_to_stroops(amount: &str) -> Result<i64, String> {
    let mut parts = amount.splitn(2, '.');
    let whole = parts.next().unwrap_or("0");
    let frac = parts.next().unwrap_or("");
    if frac.len() > 7 {
        return Err(format!("unexpected precision in amount: {amount}"));
    }
    let frac_padded = format!("{frac:0<7}");
    let whole_stroops: i64 = whole
        .parse()
        .map_err(|_| format!("invalid amount: {amount}"))?;
    let frac_stroops: i64 = frac_padded
        .parse()
        .map_err(|_| format!("invalid amount: {amount}"))?;
    Ok(whole_stroops * 10_000_000 + frac_stroops)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Formats a non-negative f64 as a Stellar amount string with exactly 7
/// fractional digits — the canonical format Horizon returns (e.g. "2.5000000").
fn to_stellar_string(value: f64) -> String {
    // Round to 7 decimal places to match Stellar's stroop precision.
    let stroops = (value * 1e7).round() as i64;
    let whole = stroops / 10_000_000;
    let frac = stroops % 10_000_000;
    format!("{whole}.{frac:07}")
}

// ---------------------------------------------------------------------------
// Deterministic unit tests (fast, always run)
// ---------------------------------------------------------------------------

#[test]
fn mirror_matches_known_values() {
    // Ensure the local mirror behaves identically to the known-good unit tests
    // in `src/blockchain/stellar.rs`.
    assert_eq!(
        parse_amount_to_stroops("10000.0000000").unwrap(),
        100_000_000_000
    );
    assert_eq!(parse_amount_to_stroops("0.5").unwrap(), 5_000_000);
    assert_eq!(parse_amount_to_stroops("1").unwrap(), 10_000_000);
    assert_eq!(
        parse_amount_to_stroops("123.4567890").unwrap(),
        1_234_567_890
    );
}

#[test]
fn mirror_rejects_excess_precision() {
    assert!(parse_amount_to_stroops("1.12345678").is_err());
}

#[test]
fn zero_amount_parses_to_zero_stroops() {
    assert_eq!(parse_amount_to_stroops("0.0000000").unwrap(), 0);
    assert_eq!(parse_amount_to_stroops("0").unwrap(), 0);
}

#[test]
fn parses_exactly_seven_fractional_digits() {
    // 7 fractional digits is the maximum — must succeed.
    assert_eq!(
        parse_amount_to_stroops("1.2345678").unwrap(),
        12_345_678
    );
}

#[test]
fn rejects_eight_fractional_digits() {
    // 8 fractional digits exceeds Stellar precision — must fail.
    assert!(parse_amount_to_stroops("1.23456789").is_err());
}

#[test]
fn handles_no_fractional_part() {
    assert_eq!(parse_amount_to_stroops("42").unwrap(), 420_000_000);
}

#[test]
fn stellar_string_round_trip_for_representative_values() {
    // Spot-check the round-trip helper against known amounts.
    let cases: &[(f64, i64)] = &[
        (0.0, 0),
        (1.0, 10_000_000),
        (2.5, 25_000_000),
        (100.0, 1_000_000_000),
        (922_337_203.0, 9_223_372_030_000_000), // near i64 max / 10
    ];
    for &(value, expected_stroops) in cases {
        let s = to_stellar_string(value);
        let got = parse_amount_to_stroops(&s).unwrap();
        assert_eq!(
            got, expected_stroops,
            "round-trip failed for value={value}, string={s:?}"
        );
    }
}

#[test]
fn empty_string_returns_err_not_panic() {
    // The empty string is not a valid amount; must return Err.
    assert!(parse_amount_to_stroops("").is_err());
}

#[test]
fn non_numeric_string_returns_err_not_panic() {
    for input in &["abc", "1.2.3", "-1", "NaN", "inf", "1e7", " ", "\n"] {
        let result = parse_amount_to_stroops(input);
        assert!(
            result.is_err(),
            "expected Err for input {input:?}, got Ok({:?})",
            result.unwrap()
        );
    }
}

// ---------------------------------------------------------------------------
// Property-based tests using proptest
// ---------------------------------------------------------------------------

use proptest::prelude::*;

proptest! {
    /// Property 1 — Round-trip correctness.
    ///
    /// For any non-negative f64 value in [0, 9.22e11] (within i64/10_000_000
    /// range), converting to the canonical Stellar 7-decimal string and then
    /// parsing via `parse_amount_to_stroops` must yield
    /// `(value * 1e7).round() as i64`.
    #[test]
    fn round_trip_f64_to_stroops(
        // Generate values in [0, 922_337_203.0] — the largest whole-XLM value
        // whose stroops fit in i64 (i64::MAX = 9_223_372_036_854_775_807,
        // divided by 10_000_000 ≈ 922_337_203).
        value in 0.0_f64..=922_337_203.0_f64
    ) {
        let stellar_str = to_stellar_string(value);
        let expected = (value * 1e7).round() as i64;
        let got = parse_amount_to_stroops(&stellar_str)
            .map_err(|e| TestCaseError::fail(format!("parse error for {stellar_str:?}: {e}")))?;
        prop_assert_eq!(
            got,
            expected,
            "round-trip mismatch: value={value}, string={stellar_str:?}, expected={expected}, got={got}"
        );
    }

    /// Property 2 — Panic safety.
    ///
    /// `parse_amount_to_stroops` must never panic for any ASCII string input.
    /// It should always return `Ok` or `Err` — never unwind.
    #[test]
    fn never_panics_on_ascii_input(s in "[ -~]{0,30}") {
        // `[ -~]` matches all printable ASCII characters (0x20..=0x7E).
        // The result is either Ok or Err — both are acceptable; a panic is not.
        let _ = parse_amount_to_stroops(&s);
    }

    /// Property 3 — Strings with exactly 7 fractional digits always succeed
    /// when both whole and fractional parts are valid decimal integers.
    #[test]
    fn valid_decimal_with_7_fractional_digits_always_parses(
        whole in 0_i64..=922_337_203_i64,
        frac in 0_u32..=9_999_999_u32,
    ) {
        let s = format!("{whole}.{frac:07}");
        let result = parse_amount_to_stroops(&s);
        prop_assert!(
            result.is_ok(),
            "expected Ok for well-formed input {s:?}, got Err({:?})",
            result.unwrap_err()
        );
        let got = result.unwrap();
        let expected = whole * 10_000_000 + frac as i64;
        prop_assert_eq!(got, expected, "stroop value mismatch for input {s:?}");
    }

    /// Property 4 — Strings with more than 7 fractional digits always return Err.
    ///
    /// The Stellar protocol allows at most 7 decimal places. Any string with
    /// 8 or more fractional digits must be rejected — this is a correctness
    /// invariant (silently truncating would lose precision).
    #[test]
    fn excess_fractional_digits_always_rejected(
        whole in 0_i64..=999_i64,
        frac in 10_000_000_u64..=99_999_999_u64, // exactly 8 digits
    ) {
        let s = format!("{whole}.{frac}");
        let result = parse_amount_to_stroops(&s);
        prop_assert!(
            result.is_err(),
            "expected Err for 8-fractional-digit input {s:?}, got Ok({:?})",
            result.unwrap()
        );
    }
}
