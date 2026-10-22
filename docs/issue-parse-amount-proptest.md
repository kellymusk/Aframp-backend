# Property-Based Tests for `parse_amount_to_stroops`

## Summary

This document describes the property-based tests designed for `parse_amount_to_stroops`
in `src/blockchain/stellar.rs`, and explains why they cannot be submitted as a CI-passing
PR from the `dev-backend` branch in their current form.

---

## The blocker

`src/blockchain/stellar.rs` and `Cargo.toml` are both in the **protected path list**
enforced by the `guard-main-source` CI job on `dev-backend` (see `.github/workflows/ci.yml`).
Any PR that touches either file will fail the guard and cannot be merged.

The issue asks contributors to:
1. Add `proptest` to `[dev-dependencies]` in `Cargo.toml` — **protected**.
2. Add tests to the existing `#[cfg(test)]` block in `src/blockchain/stellar.rs` — **protected**.

Additionally, `parse_amount_to_stroops` is a private function (`fn`, not `pub fn`), so it
cannot be called from an external integration test in `tests/` even if the contributor were
allowed to write there.

**Resolution requires a maintainer** to either:
- Merge the changes below directly on `master`, bypassing `dev-backend` (the intended flow
  for production source changes per `CONTRIBUTING.md` §"All source changes must flow through
  dev-backend and be explicitly reviewed by a maintainer"), or
- Make `parse_amount_to_stroops` `pub(crate)` and add a re-export so that an integration
  test in `tests/` can reach it, then update the guard to allow `tests/` additions that
  import `pub(crate)` items.

---

## Proposed changes

### 1. `Cargo.toml` — add `proptest` as a dev dependency

```toml
[dev-dependencies]
proptest = "1.5.0"
stellar-base = "0.7.0"
tower = { version = "0.5", features = ["util"] }
```

### 2. `src/blockchain/stellar.rs` — add two property tests

Insert the following block **inside the existing `#[cfg(test)] mod tests { … }` block**,
before the `failed_transaction_produces_no_deposit` test:

```rust
// --------------------------------------------------------------------------
// Property-based tests for parse_amount_to_stroops
// --------------------------------------------------------------------------

// Property 1 — round-trip correctness
//
// For any well-formed Stellar amount string (non-negative whole part and
// up to 7 fractional digits) the parser must return
//
//   whole * 10_000_000  +  frac_padded_to_7_digits
//
// We generate the string and expected value together to avoid floating-
// point rounding issues that would arise from going via f64.
//
// The whole part is bounded at 922_337_203 so that
// 922_337_203 * 10_000_000 = 9_223_372_030_000_000 is well under i64::MAX
// (9_223_372_036_854_775_807), leaving room for any frac contribution
// without overflow.

proptest::proptest! {
    #[test]
    fn prop_valid_amount_round_trips(
        whole in 0u64..=922_337_203u64,
        frac_digits in 0usize..=7usize,
        frac_value in 0u32..=9_999_999u32,
    ) {
        // Keep frac_value in range for the chosen number of digits.
        let max_for_digits = 10u32.saturating_pow(frac_digits as u32);
        let frac_value = if frac_digits == 0 { 0u32 } else { frac_value % max_for_digits };

        // Build the amount string exactly as Horizon produces it.
        let amount_str = if frac_digits == 0 {
            format!("{whole}")
        } else {
            // Zero-pad the frac value to exactly `frac_digits` digits so
            // that e.g. frac_digits=3, frac_value=5 → "005".
            format!("{whole}.{frac_value:0>frac_digits$}")
        };

        // Compute the expected stroops without floating point.
        let frac_str = if frac_digits == 0 {
            "0000000".to_string()
        } else {
            format!("{frac_value:0>frac_digits$}")
        };
        // Pad (or keep) to 7 digits on the right, just like the function.
        let frac_padded = format!("{frac_str:0<7}");
        let expected = (whole as i64) * 10_000_000
            + frac_padded.parse::<i64>().unwrap();

        let result = parse_amount_to_stroops(&amount_str);
        proptest::prop_assert!(
            result.is_ok(),
            "expected Ok for valid amount {amount_str:?}, got {result:?}"
        );
        proptest::prop_assert_eq!(
            result.unwrap(),
            expected,
            "stroop mismatch for {amount_str:?}"
        );
    }
}

// Property 2 — no panics on arbitrary ASCII input
//
// parse_amount_to_stroops must never panic — it must always return Ok or
// Err regardless of what it is given.  We use proptest's string regex
// strategy to generate any ASCII string (bytes 0x00–0x7E).

proptest::proptest! {
    #[test]
    fn prop_never_panics_on_arbitrary_ascii(s in "[\x00-\x7e]*") {
        // The only contract here is: no panic.
        let _ = parse_amount_to_stroops(&s);
    }
}

// --------------------------------------------------------------------------
// End of property-based tests
// --------------------------------------------------------------------------
```

---

## Why the chosen approach

### `prop_valid_amount_round_trips`

The issue suggests testing `(f64 * 1e7).round() as i64`, but going via `f64` introduces
rounding error for large values (f64 has 53-bit mantissa, and amounts like `922337203.4999999`
cannot be represented exactly). Instead, the test generates a `(whole, frac_digits, frac_value)`
triple and constructs both the input string and the expected result using only integer arithmetic
— no floating point involved, so the property is exact.

Edge cases exercised by the strategy:
- `frac_digits = 0` → no decimal point at all (e.g. `"42"`)
- `frac_digits = 1..=6` → partial precision (e.g. `"1.5"`, `"1.05"`)
- `frac_digits = 7` → maximum allowed precision (e.g. `"1.5000000"`)
- `frac_value = 0` with `frac_digits > 0` → `"1.000"` — tests zero-padding
- `whole = 0` → amounts below 1 XLM
- `whole` near `922_337_203` → amounts close to `i64::MAX / 10_000_000`

### `prop_never_panics_on_arbitrary_ascii`

Rust's `unwrap()` calls inside `parse_amount_to_stroops` are guarded by `map_err(...)`, so
the function should always return `Ok` or `Err`. This property confirms that holds for any
ASCII input — including adversarial inputs like `"."`, `"0.0000000.0"`, `""`, `" "`,
`"\x00"`, very long strings, etc.

---

## Files that would be modified (requires maintainer)

| File | Change |
|------|--------|
| `Cargo.toml` | Add `proptest = "1.5.0"` to `[dev-dependencies]` |
| `src/blockchain/stellar.rs` | Add two `proptest::proptest!` blocks to `#[cfg(test)] mod tests` |
