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
```

## Questions?

Open a Discussion or comment on the relevant issue.
