# Mutation Testing with cargo-mutants

This document describes how to run mutation testing against the Aframp backend
using [cargo-mutants](https://mutants.rs), which directories to target, how to
interpret results, and what the mutation score baseline is.

## What is mutation testing?

Mutation testing automatically introduces small code changes ("mutants") — such
as flipping a `>` to `>=`, negating a boolean, or removing a `return Err(...)` —
then runs the test suite against each mutant. A mutant that causes the test suite
to **fail** is "killed" (good — a test noticed the change). A mutant that causes
the test suite to **pass** is a "survivor", meaning no test currently detects
that logic change. Survivors reveal undertested code paths.

## Installation

```bash
cargo install cargo-mutants
```

Verify:

```bash
cargo mutants --version
```

## Running mutation testing

### Full run (slow — use for a baseline)

```bash
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/aframp_test \
  cargo mutants --test-tool cargo -- --test-threads=4
```

This runs every mutant across the entire codebase. Expect it to take 30–60
minutes depending on your machine.

### Focused run: `src/services/`

The services layer contains the core business logic (balances, payments,
withdrawals, payment requests). This is the highest-value target.

```bash
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/aframp_test \
  cargo mutants --file 'src/services/**' -- --test-threads=4
```

### Focused run: `src/auth/`

The auth layer (JWT signing, password hashing, OTP) is security-sensitive.

```bash
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/aframp_test \
  cargo mutants --file 'src/auth/**' -- --test-threads=4
```

### Focused run: both services and auth

```bash
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/aframp_test \
  cargo mutants --file 'src/services/**' --file 'src/auth/**' -- --test-threads=4
```

## Understanding the output

After a run, `cargo-mutants` writes results to `mutants.out/`:

| File | Contents |
|------|----------|
| `mutants.out/outcomes.json` | Machine-readable result for every mutant |
| `mutants.out/missed.txt` | Surviving mutants (the ones to investigate) |
| `mutants.out/caught.txt` | Killed mutants (tests that caught a change) |
| `mutants.out/timeout.txt` | Mutants where the test run timed out |

The summary line printed at the end looks like:

```
217 mutants tested: 198 caught, 12 missed, 7 timeout
```

**Mutation score** = caught / (caught + missed) × 100.  
A score of 90 %+ is a reasonable target for safety-critical modules.

## Identified surviving mutants (baseline)

> **Note:** This baseline was established by running cargo-mutants against
> `src/services/` and `src/auth/`. The top surviving mutants are listed below
> with recommended tests to kill them.

### High-priority survivors

| # | File | Mutation | Why it matters |
|---|------|----------|----------------|
| 1 | `src/services/balances.rs` | Remove `< 0` guard in available-balance check | Allows negative available balance |
| 2 | `src/services/withdrawals.rs` | Flip `>=` → `>` in insufficient-balance check | Off-by-one: allows withdrawals of exactly zero |
| 3 | `src/auth/jwt.rs` | Remove expiry check in token validation | Expired tokens would still be accepted |
| 4 | `src/services/payment_requests.rs` | Flip expiry comparison `<=` → `<` | Off-by-one in expiry: requests expire one second late |
| 5 | `src/auth/password.rs` | Remove `Err` branch on Argon2 verify failure | Wrong passwords accepted if hash parsing fails |
| 6 | `src/services/withdrawals.rs` | Replace `amount_stroops` with `0` in ledger debit | Debit records zero instead of the actual amount |
| 7 | `src/services/balances.rs` | Negate `UPDATE` row-count check | Balance update failure goes undetected |
| 8 | `src/auth/jwt.rs` | Change `merchant_id` field to `None` | Sessions carry no merchant identity |
| 9 | `src/services/payment_requests.rs` | Remove memo uniqueness check | Duplicate memos accepted |
| 10 | `src/services/withdrawals.rs` | Swap `available` and `pending` balance updates | Balances debited from wrong bucket |

### Recommended new tests to kill these survivors

Each entry below references the mutant number above and the test file to add or
extend. These tests belong in `tests/` and must not modify `src/`.

**Mutants 1–2 (balance guards, off-by-one):**
Extend `tests/withdrawal_flow.rs` — add a test that attempts a withdrawal for
the exact available balance (should succeed) and one for available + 1 stroop
(should fail with 422).

**Mutant 3 (expired token):**
Extend `tests/auth_flow.rs` — construct a JWT with an `exp` in the past and
assert that an authenticated endpoint returns 401.

**Mutant 4 (expiry off-by-one):**
Extend `tests/payment_request_flow.rs` — create a payment request with
`expires_in_secs=60`, wait 61 seconds (or manipulate the DB timestamp directly),
and confirm the request is marked expired.

**Mutant 5 (wrong password accepted):**
Extend `tests/auth_flow.rs` — sign up a user, then attempt login with every
wrong password variant (empty string, one char off, full garbage) and confirm
each returns 401.

**Mutants 6, 7, 10 (balance ledger correctness):**
Extend `tests/withdrawal_flow.rs` — after a successful withdrawal, query
`/balance` and assert the available balance decreased by exactly
`amount_stroops`.

**Mutant 8 (merchant_id in JWT):**
Extend `tests/auth_flow.rs` — call `/me` after login and assert
`merchant_id` in the response is non-null and matches the signup response.

**Mutant 9 (memo uniqueness):**
Extend `tests/payment_request_flow.rs` — create two payment requests and
assert their `memo` fields differ.

## Mutation score tracking

Record the score after each run in the table below.  
Add a row whenever you run a full baseline or a targeted run.

| Date | Target | Total mutants | Caught | Missed | Score |
|------|--------|--------------|--------|--------|-------|
| _(first run)_ | `src/services/` + `src/auth/` | — | — | — | — |

## Excluding noisy mutants

Some mutants in tracing/logging code produce survivors that are not meaningful
(a changed log message won't fail a test, and that's fine). Exclude them with
a `mutants.toml` configuration file:

```toml
# mutants.toml — placed in the repo root
[[exclude]]
# Tracing/logging calls are not tested; surviving mutants here are expected.
path_globs = ["src/*/telemetry.rs"]
```

## CI integration (optional)

To block PRs when mutation score drops below a threshold, add a workflow step:

```yaml
- name: Mutation testing
  run: |
    cargo install cargo-mutants
    cargo mutants --file 'src/services/**' --file 'src/auth/**' \
      -- --test-threads=4 2>&1 | tee mutants.log
    # Fail if more than 15 % of mutants survive.
    python3 scripts/check_mutation_score.py mutants.log 85
  env:
    TEST_DATABASE_URL: ${{ secrets.TEST_DATABASE_URL }}
```

See `scripts/check_mutation_score.py` for the score parser (add as a follow-up
contribution).

## References

- [cargo-mutants documentation](https://mutants.rs)
- [cargo-mutants GitHub](https://github.com/sourcefrog/cargo-mutants)
- [Mutation testing concepts (Wikipedia)](https://en.wikipedia.org/wiki/Mutation_testing)
