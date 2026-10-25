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

```bash
docker exec -i aframp-postgres psql -U postgres -c "CREATE DATABASE aframp_test;"
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/aframp_test cargo test
```

## Questions?

Open a Discussion or comment on the relevant issue.
