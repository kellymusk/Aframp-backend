# Project

## Configuration

The application is configured through environment variables. Copy `.env.example` to `.env` and adjust the values as needed.

### Database

| Variable | Description | Default |
| --- | --- | --- |
| `DATABASE_URL` | PostgreSQL connection string used to build the connection pool. | — |
| `DATABASE_MAX_CONNECTIONS` | Maximum number of connections the `PgPool` may open. Increase this for concurrent workloads (e.g. simultaneous withdrawals while the poll worker runs). | `10` |
| `DATABASE_MIN_CONNECTIONS` | Minimum number of idle connections the `PgPool` keeps open. | `2` |

Both pool sizing variables are read in `src/lib.rs::build_state` and passed to `PgPoolOptions`. If they are unset or invalid, the defaults above are used.
