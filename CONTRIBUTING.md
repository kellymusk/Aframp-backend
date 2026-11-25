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

---

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

### Middleware scope

Every new route registered in `src/lib.rs` (or `src/main.rs`) must be added to
`tests/middleware_scope.rs` in the same PR. For each route, list it together
with its expected auth scope and assert the matching behaviour:

| Scope | Expected behaviour without credentials |
|-------|----------------------------------------|
| `public` | `200` (or the route's normal success status) |
| `merchant-auth` | `401` |
| `admin-auth` | `401` / `403` |

See `docs/middleware-scope-refactor.md` for the middleware architecture. A route
that lands in the wrong scope is a security bug, and the middleware scope test
is what catches it — do not skip this step when adding routes.

## Reporting Issues

Use the GitHub issue tracker. See `.github/ISSUE_TEMPLATE/` for templates covering bugs, feature requests, and security vulnerabilities.

**Security issues**: follow the process in `SECURITY.md` — do not open a public issue for a vulnerability.

## Questions

Open a discussion or comment on the relevant issue.
