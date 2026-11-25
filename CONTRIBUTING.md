# Contributing to Aframp Backend

Thank you for your interest in contributing! This document explains how to contribute safely without touching the existing production backend.

---

## Contributor Sandbox (`dev-backend` branch)

All open-source contributions happen on the **`dev-backend`** branch. A CI guard job (`Guard`) runs on every push and pull request to that branch. **If your changes touch any file outside the sandbox, the CI will fail and your PR cannot be merged.**

### What you CAN change on `dev-backend`

| Path | Purpose |
|------|---------|
| `tests/` | New or updated integration and unit tests |
| `docs/` | Documentation, ADRs, proposals |
| `scripts/` | Helper scripts and benchmarks |
| `API.md` | Public API documentation |
| `openapi.yaml` | OpenAPI specification |
| `README.md` | Project README |
| `CHANGELOG.md` | Changelog entries |

### What you CANNOT change on `dev-backend`

Everything else is protected:

- `src/` — production Rust source code
- `migrations/` — database schema migrations
- `Cargo.toml` / `Cargo.lock` — dependency manifest
- `Dockerfile` — container build
- `.github/workflows/` — CI/CD pipelines
- `.github/CODEOWNERS` — code ownership rules
- `SECURITY.md` — security policy

Changes to protected paths require a maintainer-authored PR against the **`main`** branch with an approved code review from `@kelly-musk`.

### Do not modify `Cargo.toml` or `Cargo.lock` on `dev-backend`

`Cargo.toml` and `Cargo.lock` are protected paths. On `dev-backend` you must
**not** add, remove, or update dependencies, and you must **not** commit any
change to `Cargo.lock`.

A common accident: running `cargo add <crate>` or `cargo update` while working
in `tests/` rewrites `Cargo.lock` even though you never meant to change
dependencies. If that happens, your PR will fail CI with the **`Cargo.lock up to
date`** check (the `cargo-lock-check` workflow, `.github/workflows/cargo-lock-check.yml`),
and the `Guard` job will flag `Cargo.lock` as a protected-path violation.

**How to recover:**

```bash
# Discard the accidental lock file change
git checkout -- Cargo.lock

# If you already committed it, revert the commit or restore the file:
git checkout HEAD~1 -- Cargo.lock
git commit -m "revert accidental Cargo.lock change"
```

Then push again — the `Cargo.lock up to date` check should pass. If you believe
a dependency change is genuinely required, open an issue and let a maintainer
handle it in a PR against `main`.

---
=======

## Getting Started

1. Fork the repository
2. Clone your fork
3. Checkout `dev-backend`: `git checkout dev-backend`
4. Create a feature branch off `dev-backend`: `git checkout -b my-feature`
5. Make your changes (sandbox paths only)
6. Run tests: `cargo test`
7. Open a PR targeting `dev-backend`

## Running Tests

```bash
# All tests (requires DATABASE_URL)
cargo test

# A specific test file
cargo test --test auth_flow

# With output
cargo test -- --nocapture
```

## Code Style

- Run `cargo fmt` before committing
- Run `cargo clippy` and address all warnings
- Keep test names descriptive: `test_name_describes_what_it_asserts`

### OpenAPI security declarations

Every new endpoint in `openapi.yaml` must declare its auth. Authenticated
endpoints inherit the global `security` (cookieAuth / bearerAuth); admin
endpoints must set `security: [{ adminBearerAuth: [] }]`; public endpoints must
set `security: []` explicitly.

## Reporting Issues

Use the GitHub issue tracker. See `.github/ISSUE_TEMPLATE/` for templates covering bugs, feature requests, and security vulnerabilities.

**Security issues**: follow the process in `SECURITY.md` — do not open a public issue for a vulnerability.

## Questions

Open a discussion or comment on the relevant issue.
