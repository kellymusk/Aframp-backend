# Contributing to Aframp Backend (dev-backend)

Welcome! This branch (`dev-backend`) is where all open-source contributions happen.

## How this branch works

The `dev-backend` branch is a **contributor sandbox** on top of the production backend.

- The main backend source code lives in `src/`, `migrations/`, `Cargo.toml`, `Dockerfile`, etc.
- **Contributors must not modify those files.** A CI check enforces this — if your PR touches any of those paths, the `Guard` job will fail immediately.
- You can freely add or modify anything outside the protected paths: `tests/`, `docs/`, `scripts/`, new `.github/workflows/`, `API.md`, `openapi.yaml`, `README.md`, etc.

## Protected paths (do not modify)

| Path | Reason |
|------|--------|
| `src/` | Production Rust source |
| `migrations/` | Database schema — changes here affect live data |
| `examples/` | Testnet proof harness |
| `Dockerfile` | Container build |
| `Cargo.toml` / `Cargo.lock` | Dependency manifest |
| `wrangler.jsonc` | Cloudflare deployment config |
| `cloudflare/` | Cloudflare Worker source |

## What you CAN work on

All 100 issues in this repo are fair game. Each one is grounded in a real gap, bug, or missing feature found in the code. To contribute:

1. Pick an open issue.
2. Comment on it to claim it (avoids duplicate work).
3. Branch off `dev-backend`: `git checkout -b fix/your-issue-name`.
4. Write your code — integration tests, documentation, scripts, etc.
5. Open a PR targeting `dev-backend`.
6. CI must be green (guard + build + lint) before review.

## Code review process

There are **two independent layers** of review on `dev-backend`. Both must pass
before a pull request can be merged.

### Layer 1 — automated `Guard` job (`.github/workflows/ci.yml`)

The `Guard — no touches to main backend source` job diffs your PR against the base
branch and fails immediately if any changed file matches a protected pattern
(`^src/`, `^migrations/`, `^examples/`, `^Dockerfile$`, `^Cargo.toml$`,
`^Cargo.lock$`, `^wrangler.jsonc$`, `^cloudflare/`).

This job is a hard block, not a warning. It is the machine-enforced half of the
contributor sandbox and it cannot be bypassed by a contributor.

### Layer 2 — human review via CODEOWNERS

`.github/CODEOWNERS` marks the paths that are too sensitive to merge on
automation alone. When a PR touches any of them, GitHub automatically requests
a review from the listed owners and the PR cannot be merged until they approve.

Paths currently under CODEOWNERS:

| Path | Why it needs a human |
|------|----------------------|
| `src/auth/` | Session, JWT and password handling — account takeover surface |
| `src/blockchain/` | Stellar listener, key handling and `wallet_crypto.rs` |
| `migrations/` | Schema changes are one-way doors on live data |
| `src/services/withdrawals.rs` | Direct custodial fund movement |
| `src/services/payments.rs`, `src/services/payment_requests.rs` | Payment execution and memo correlation |
| `src/services/otp.rs`, `src/otp/` | OTP issuance, hashing and phone verification |
| `src/api/webhooks.rs` | Untrusted ingress and signature verification |
| `src/main.rs`, `src/lib.rs` | Process entrypoint and shared wiring |
| `.github/workflows/`, `.github/CODEOWNERS` | A change here can weaken the guardrails for everyone |
| `SECURITY.md`, `CONTRIBUTING.md` | Contribution policy |

**The most specific matching rule wins**, so `src/blockchain/wallet_crypto.rs`
can be called out explicitly even though it also matches `src/blockchain/`.
Entries are listed most-specific-last, as GitHub evaluates top to bottom.

Everything not matched — `tests/`, `docs/`, `scripts/`, `API.md`,
`openapi.yaml`, `README.md` — falls back to the default owner in the last rule
that applies, and only needs the normal community review.

### What reviewers check

For a contributor PR (tests, docs, scripts):

1. The `Guard` job is green.
2. Build, coverage (60% line minimum), snapshot tests and `lint` are green.
3. Tests actually exercise the behaviour they claim to — no assertions that
   pass vacuously.
4. `openapi.yaml` / `API.md` updated if a documented contract changed.

For a change under CODEOWNERS (maintainer-only paths):

1. Everything above, plus:
2. Does this touch authentication, key material, or fund movement?
3. Is there a migration, and is it reversible or explicitly one-way?
4. Are secrets, PII, or internal identifiers absent from the diff and test fixtures?
5. Is the failure mode safe — does the change fail closed?
6. Is the blast radius understood for live custodial funds?

### Handling a review

- PRs are reviewed in the order they are submitted; older PRs with an approved
  review go first.
- Request changes with a concrete, actionable reason — "this assertion can
  never fail because the loop is empty" beats "please fix".
- Address feedback with a follow-up commit, then re-request review. Do not force-push
  over commits a reviewer has already commented on.
- Once a PR is approved, only the approving maintainer (or another CODEOWNER)
  should merge.

## Running locally

```bash
cp .env.example .env
# fill in .env

docker run -d --name aframp-postgres \
  -e POSTGRES_USER=postgres -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=aframp \
  -p 5432:5432 postgres:16

for f in migrations/*.sql; do
  docker exec -i aframp-postgres psql -U postgres -d aframp < "$f"
done

cargo run
```

## Running tests

Integration tests (everything under `tests/`) need a dedicated Postgres
database. They **fail** — they do not skip — when it isn't configured, so a
green run always means the tests actually ran.

```bash
docker exec -i aframp-postgres psql -U postgres -c "CREATE DATABASE aframp_test;"
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/aframp_test cargo test
```

## Standard make targets

Use the project's `Makefile` for common commands:

```bash
make run        # OTP_PROVIDER=mock cargo run
make test       # cargo test (uses TEST_DATABASE_URL, defaulting to local aframp_test)
make migrate    # sqlx migrate run (uses DATABASE_URL, defaulting to local aframp)
make fmt        # cargo fmt --all
make clippy     # cargo clippy -- -D warnings
make docker     # docker build -t aframp-backend .
make docker-db  # start (or create) local Postgres container
## Mock OTP provider (`OTP_PROVIDER=mock`)

Local and CI runs use `MockOtpProvider` (`src/otp/mock.rs`) instead of Termii.

| Behaviour | Detail |
|---|---|
| `send_sms` | Logs the message and stores it in-memory; tests read it back with `aframp::otp::mock::last_message_for(phone)` |
| Webhook signature | `verify_webhook_signature` accepts **exactly** the sentinel string `mock-signature` (any other value → `false`) |
| Termii webhook tests | Send `X-Termii-Signature: mock-signature` for a `204`; any other signature (or missing header) → `403 FORBIDDEN` |

This is the configurable expected signature for tests: hard-coded sentinel rather than HMAC, so integration tests for `POST /webhooks/termii` work without a real Termii API key. See `tests/webhook_flow.rs`.
### Required environment variables

| Variable | Required for | Notes |
|----------|--------------|-------|
| `TEST_DATABASE_URL` | All integration tests in `tests/` | Must point at a database you don't mind being written to. Migrations are applied automatically by the test helper (`tests/common/mod.rs`) — don't apply them by hand with `psql`, or the helper's migration run will fail on already-existing tables. |

Everything else the tests need (JWT/webhook/OTP secrets, the wallet
encryption key, the payment and OTP providers) is hard-coded to test values
or mocks inside `tests/common/mod.rs`, so no other variables are required to
run `cargo test` locally.

If `TEST_DATABASE_URL` is missing or unreachable, every integration test —
and the `tests/db_canary.rs` canary — fails with a message pointing back
here. To run only the unit tests without a database:

```bash
cargo test --lib
```

## Pre-commit hooks (optional but recommended)

A `.pre-commit-config.yaml` is included in the repository root. It runs
`cargo fmt --check` and `cargo clippy -- -D warnings` locally before each
commit, catching the same failures the CI `lint` job catches — before they
ever reach the remote.

The hooks are **opt-in**: nothing breaks if you don't install them. They are
recommended for contributors who want faster feedback.

### Setup

1. Install the `pre-commit` tool (Python-based, works on macOS, Linux, and
   Windows):

   ```bash
   pip install pre-commit
   # or, if you use Homebrew:
   brew install pre-commit
   ```

2. Install the hooks into your local clone:

   ```bash
   pre-commit install
   ```

   This writes a `.git/hooks/pre-commit` script that runs the configured
   hooks automatically on every `git commit`.

3. (Optional) Run all hooks against every file without making a commit:

   ```bash
   pre-commit run --all-files
   ```

### What the hooks do

| Hook | Command | When it fails |
|------|---------|---------------|
| `cargo-fmt-check` | `cargo fmt --all -- --check` | Any Rust file is not formatted. Run `cargo fmt --all` to fix. |
| `cargo-clippy` | `cargo clippy -- -D warnings` | Any clippy lint warning is present. Fix the warning or, if intentional, add `#[allow(...)]` in the source. |

### Skipping hooks

If you need to bypass the hooks for a specific commit (e.g. a work-in-progress
commit on a local branch that you plan to `--amend` before pushing):

```bash
git commit --no-verify
```

This should not be used on commits you intend to push upstream — CI will
catch the same failures and your PR will be blocked.

### Keeping hooks up to date

```bash
pre-commit autoupdate
```

## Questions?

Open a Discussion or comment on the relevant issue.
