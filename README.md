# Project

[![CI](https://github.com/OWNER/REPO/actions/workflows/ci.yml/badge.svg?branch=dev-backend)](https://github.com/OWNER/REPO/actions/workflows/ci.yml?query=branch%3Adev-backend)

## Configuration

The application is configured through environment variables. Copy `.env.example` to `.env` and adjust the values as needed.

### Database

| Variable | Description | Default |
| --- | --- | --- |
| `DATABASE_URL` | PostgreSQL connection string used to build the connection pool. | — |
| `DATABASE_MAX_CONNECTIONS` | Maximum number of connections the `PgPool` may open. Increase this for concurrent workloads (e.g. simultaneous withdrawals while the poll worker runs). | `10` |
| `DATABASE_MIN_CONNECTIONS` | Minimum number of idle connections the `PgPool` keeps open. | `2` |

Both pool sizing variables are read in `src/lib.rs::build_state` and passed to `PgPoolOptions`. If they are unset or invalid, the defaults above are used.

### Tests

Integration tests require a reachable PostgreSQL instance via `TEST_DATABASE_URL`. If it is unset or unreachable, the integration suite is skipped locally. CI (`.github/workflows/ci.yml`) asserts `TEST_DATABASE_URL` is reachable, runs migrations, and fails if no integration tests execute, so a green CI run means the integration suite actually ran.
