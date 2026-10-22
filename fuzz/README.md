# Fuzz Testing for Aframp

This directory contains fuzz targets for HTTP request body parsing using cargo-fuzz.

## Prerequisites

Install cargo-fuzz:

```bash
cargo install cargo-fuzz
```

## Running Fuzz Tests

Run individual fuzz targets:

```bash
# Fuzz signup endpoint JSON parsing
cargo fuzz run fuzz_signup

# Fuzz login endpoint JSON parsing
cargo fuzz run fuzz_login

# Fuzz payment-request endpoint JSON parsing
cargo fuzz run fuzz_payment_request
```

## Fuzz Targets

### fuzz_signup
Tests JSON body parsing for the `/signup` endpoint:
- email (String)
- password (String)  
- name (String)

### fuzz_login
Tests JSON body parsing for the `/login` endpoint:
- email (String)
- password (String)

### fuzz_payment_request
Tests JSON body parsing for the `/payment-requests` endpoint:
- amount_stroops (i64)
- asset (Optional<String>)
- expires_in_secs (Optional<i64>)

## Corpus

Fuzz corpora are stored in `fuzz/corpus/<target_name>/` and artifacts (crashes) in `fuzz/artifacts/<target_name>/`.

## Notes

- These fuzz targets focus on JSON deserialization robustness
- They do not test business logic or database interactions
- Run with `--jobs` flag for parallel fuzzing: `cargo fuzz run fuzz_signup -- -jobs=4`
- Set time limits: `cargo fuzz run fuzz_signup -- -max_total_time=300` (5 minutes)
