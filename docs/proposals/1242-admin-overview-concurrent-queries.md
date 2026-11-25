# #1242 — Run `admin::overview()` queries concurrently

**Status:** implemented (see #1485)

## Problem

`src/services/admin.rs::overview()` issues 7 independent queries. When awaited
sequentially, total latency is the *sum* of 7 round-trips.

## Implemented change

The queries share no data, so they now run concurrently with `tokio::try_join!`.
Each future checks out its own pool connection; `try_join!` short-circuits on
the first error, so any failure still returns `Err` exactly as today.

The implementation runs the queries in **two waves**:

1. **Wave 1 — the three scalar counts** (`users`, `merchants`, `wallets`) are
   joined together in a single `tokio::try_join!`.
2. **Wave 2 — the four grouped aggregates** (`balances_by_asset`,
   `payments_by_status`, `withdrawals_by_status`,
   `payment_requests_by_status`) are joined together in a second
   `tokio::try_join!`.

Splitting into two waves keeps the peak number of in-flight queries at 4, which
fits comfortably inside the default pool size (see the caveat below) while still
cutting the sequential 7 round-trips down to 2.

```rust
pub async fn overview(db: &PgPool) -> Result<AdminOverview, sqlx::Error> {
    // Wave 1: scalar counts.
    let (total_users, total_merchants, total_wallets) = tokio::try_join!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users").fetch_one(db),
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM merchants").fetch_one(db),
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM wallets").fetch_one(db),
    )?;

    // Wave 2: grouped aggregates.
    let (
        balances_by_asset,
        payments_by_status,
        withdrawals_by_status,
        payment_requests_by_status,
    ) = tokio::try_join!(
        sqlx::query_as::<_, AssetTotal>(
            "SELECT asset, coalesce(sum(available), 0) AS available, coalesce(sum(pending), 0) AS pending
             FROM balances GROUP BY asset ORDER BY asset",
        )
        .fetch_all(db),
        sqlx::query_as::<_, StatusCount>(
            "SELECT status, count(*) FROM payments GROUP BY status ORDER BY status",
        )
        .fetch_all(db),
        sqlx::query_as::<_, StatusCount>(
            "SELECT status, count(*) FROM withdrawals GROUP BY status ORDER BY status",
        )
        .fetch_all(db),
        sqlx::query_as::<_, StatusCount>(
            "SELECT status, count(*) FROM payment_requests GROUP BY status ORDER BY status",
        )
        .fetch_all(db),
    )?;

    Ok(AdminOverview {
        total_users,
        total_merchants,
        total_wallets,
        balances_by_asset,
        payments_by_status,
        withdrawals_by_status,
        payment_requests_by_status,
    })
}
```

The three scalar counts could alternatively collapse into one round-trip:

```sql
SELECT (SELECT count(*) FROM users), (SELECT count(*) FROM merchants), (SELECT count(*) FROM wallets)
```

That would reduce wave 1 to a single query, but is not required for the current
implementation.

## Caveat: pool size

Concurrency is capped by `DB_MAX_CONNECTIONS` (default `5` in `.env.example`).
The two-wave layout keeps at most 4 queries in flight at once, so it fits within
the default pool without any query waiting on a free connection. If the pool is
configured smaller than 4, the extra futures simply wait for a connection — still
faster than fully sequential, and other requests are not starved for long since
every query is a cheap aggregate.

## Expected latency

| Mode | Latency (≈ RTT = network round-trip + query time) |
|------|------------------------------|
| Sequential (before) | 7 × RTT |
| Two-wave `try_join!` (implemented) | ≈ 2 × RTT |
| Single-wave `try_join!`, pool ≥ 7 | ≈ 1 × max(RTT) |

Against a managed Postgres with ~20 ms RTT, that's roughly 140 ms → 40 ms.
Measure with `hyperfine` against `GET /admin/overview` before and after.

## Tests

`tests/admin_overview.rs` covers the change:

- **Counts:** asserts the overview endpoint returns the correct user, merchant
  and wallet counts, plus the grouped payment/withdrawal totals, for a seeded
  fixture.
- **Non-blocking behaviour:** uses a mock/slow-DB approach to make one overview
  query slow and asserts the other queries are not serialized behind it (i.e.
  the endpoint's latency stays close to the slowest single query rather than the
  sum of all queries).

## Error handling

`try_join!` returns the first `sqlx::Error` and drops the remaining futures in
that wave, so the handler's existing `?`/500 mapping is unchanged. A failure in
wave 1 short-circuits before wave 2 runs.
