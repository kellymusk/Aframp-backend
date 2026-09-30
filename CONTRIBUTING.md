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
