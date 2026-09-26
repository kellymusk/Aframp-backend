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

```bash
docker exec -i aframp-postgres psql -U postgres -c "CREATE DATABASE aframp_test;"
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/aframp_test cargo test
```

## Mock OTP provider (`OTP_PROVIDER=mock`)

Local and CI runs use `MockOtpProvider` (`src/otp/mock.rs`) instead of Termii.

| Behaviour | Detail |
|---|---|
| `send_sms` | Logs the message and stores it in-memory; tests read it back with `aframp::otp::mock::last_message_for(phone)` |
| Webhook signature | `verify_webhook_signature` accepts **exactly** the sentinel string `mock-signature` (any other value → `false`) |
| Termii webhook tests | Send `X-Termii-Signature: mock-signature` for a `204`; any other signature (or missing header) → `403 FORBIDDEN` |

This is the configurable expected signature for tests: hard-coded sentinel rather than HMAC, so integration tests for `POST /webhooks/termii` work without a real Termii API key. See `tests/webhook_flow.rs`.

## Questions?

Open a Discussion or comment on the relevant issue.
