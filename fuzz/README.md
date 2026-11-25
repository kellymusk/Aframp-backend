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

## Lock file check

This directory also contains a regular (non-fuzz) integration test at `fuzz/tests/lockfile.rs` that guards against unapproved dependency changes (see issue #1489).

### What it verifies

- `Cargo.lock` is committed to the repository (it exists and is tracked).
- `Cargo.lock` is up-to-date and satisfies the dependency requirements declared in `Cargo.toml` (i.e. the lock file is consistent with the manifest and has not drifted).

### Why it exists

On the `dev-backend` branch, `Cargo.toml` and `Cargo.lock` are protected paths. A contributor who edits `tests/` and accidentally runs `cargo add` (or otherwise triggers a dependency resolution) will modify `Cargo.lock`, which the Guard CI job flags as a protected-path violation. This test provides an early, local signal that the lock file has changed or drifted, so the problem is caught before CI fails. See `CONTRIBUTING.md` for the policy: do not modify `Cargo.toml` or `Cargo.lock` on `dev-backend`.

### How to run it

This is a regular test, **not** a fuzz target — run it with `cargo test`, not `cargo fuzz`:

```bash
cargo test --manifest-path fuzz/Cargo.toml --test lockfile
```

## Notes

- These fuzz targets focus on JSON deserialization robustness
- They do not test business logic or database interactions
- Run with `--jobs` flag for parallel fuzzing: `cargo fuzz run fuzz_signup -- -jobs=4`
- Set time limits: `cargo fuzz run fuzz_signup -- -max_total_time=300` (5 minutes)
- The lock file check (`fuzz/tests/lockfile.rs`) is a standard integration test, not a fuzz target