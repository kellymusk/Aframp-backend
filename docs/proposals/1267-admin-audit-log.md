# Proposal: Durable Admin Audit Log (#1267)

## Problem

In `src/auth/extractor.rs`, `AdminUser::from_request_parts` logs:

```rust
tracing::info!(admin_user_id = %claims.sub, path = %parts.uri.path(), "admin access");
```

This is a structured tracing log — not stored in the database. If the log
aggregator is not configured or logs are rotated, there is no durable record of
admin actions. An admin who reads sensitive data or modifies a user row leaves
no audit trail.

## Proposed solution

Insert a row into a new `admin_audit_log` table for every admin endpoint call,
in addition to (not instead of) the existing `tracing::info!`.

### Schema

See `scripts/proposed_0010_admin_audit_log.sql` for the exact DDL.

```sql
CREATE TABLE admin_audit_log (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    admin_user_id   UUID        NOT NULL REFERENCES users(id) ON DELETE SET NULL,
    action          TEXT        NOT NULL,       -- e.g. "GET /admin/users"
    resource_type   TEXT,                       -- e.g. "users", "wallets"
    resource_id     UUID,                       -- path param, NULL for lists
    ip_address      TEXT,                       -- from X-Forwarded-For / peer
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

Three indexes:
- `admin_user_id` — "show me all actions by admin X"
- `(resource_type, resource_id)` — "show me all accesses to user Y"
- `created_at DESC` — time-ordered audit trail

### Insertion point

`AdminUser::from_request_parts` in `src/auth/extractor.rs` already validates
the admin claim and re-reads the DB. After the existing `tracing::info!`, call
a helper that fires-and-forgets the insert:

```rust
// After: tracing::info!(admin_user_id = %claims.sub, path = %parts.uri.path(), "admin access");
insert_audit_log(state, claims.sub, parts).await;
```

where `insert_audit_log` is:

```rust
async fn insert_audit_log(state: &AppState, admin_user_id: Uuid, parts: &Parts) {
    let method = parts.method.as_str();
    let path   = parts.uri.path();
    let action = format!("{method} {path}");

    // resource_type derived from path, e.g. "/admin/users/..." → "users"
    let resource_type = path
        .strip_prefix("/admin/")
        .map(|rest| rest.split('/').next().unwrap_or("unknown").to_string());

    // resource_id: the segment after the resource_type, if it's a UUID.
    let resource_id: Option<Uuid> = path
        .split('/')
        .filter_map(|s| Uuid::parse_str(s).ok())
        .next();

    // IP from X-Forwarded-For (first hop) or direct peer address.
    let ip_address = parts
        .headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string());

    let _ = sqlx::query(
        "INSERT INTO admin_audit_log \
         (admin_user_id, action, resource_type, resource_id, ip_address) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(admin_user_id)
    .bind(&action)
    .bind(&resource_type)
    .bind(resource_id)
    .bind(&ip_address)
    .execute(&state.db)
    .await;
    // Failure to log must not block the admin request; the tracing log
    // is the fallback (belt-and-suspenders).
}
```

The `let _ = ...` intentionally swallows DB errors so an audit-log failure
never surfaces to the admin user or breaks the endpoint. The `tracing::info!`
log line remains for belt-and-suspenders coverage.

### Admin endpoint coverage

Every route under the `AdminUser` extractor is automatically covered because
the insert is in the extractor itself, not in individual handlers. The covered
endpoints are:

| Method | Path | resource_type |
|--------|------|---------------|
| GET | /admin/overview | overview |
| GET | /admin/merchants | merchants |
| GET | /admin/users | users |
| GET | /admin/wallets | wallets |
| GET | /admin/transactions | transactions |
| GET | /admin/withdrawals | withdrawals |
| GET | /admin/payment-requests | payment-requests |
| POST | /admin/merchants/:id/suspend | merchants |
| POST | /admin/merchants/:id/unsuspend | merchants |
| POST | /admin/users/:id/promote | users |

### Testing

`tests/admin_audit_log_test.rs` verifies:

1. Every admin endpoint call inserts a row into `admin_audit_log`.
2. The row carries the correct `admin_user_id`, `action`, and `resource_type`.
3. Non-admin requests do not insert rows.
4. A DB failure in the audit insert does not break the admin endpoint.

### Migration

Copy `scripts/proposed_0010_admin_audit_log.sql` to
`migrations/0010_admin_audit_log.sql` (or the next free number). The migration
is additive-only (new table + indexes) and fully reversible with `DROP TABLE
admin_audit_log`.

## Alternatives considered

- **PostgreSQL-level trigger** — would require modifying `migrations/` (protected)
  and a trigger function. Rejected: the application layer is the right place to
  capture semantic context (resource type, admin identity from JWT).
- **Log-shipping only** — the existing tracing log is not durable. Out-of-process
  log aggregation is a config concern, not an application guarantee.
- **Separate audit service** — premature; a local table is sufficient at
  Aframp's current scale and avoids a network dependency in the auth hot path.
