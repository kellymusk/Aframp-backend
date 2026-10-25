# #1242 — Run `admin::overview()` queries concurrently

**Status:** proposed (needs a maintainer — `src/` is protected for contributors)

## Problem

`src/services/admin.rs::overview()` issues 7 independent queries, each awaited
sequentially. Total latency is the *sum* of 7 round-trips.

## Proposed change

The queries share no data, so they can run concurrently with `tokio::try_join!`.
Each future checks out its own pool connection; `try_join!` short-circuits on
the first error, so any failure still returns `Err` exactly as today.

```rust
pub async fn overview(db: &PgPool) -> Result<AdminOverview, sqlx::Error> {
    let (
        total_users,
        total_merchants,
        total_wallets,
        balances_by_asset,
        payments_by_status,
        withdrawals_by_status,
        payment_requests_by_status,
    ) = tokio::try_join!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users").fetch_one(db),
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM merchants").fetch_one(db),
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM wallets").fetch_one(db),
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

## Caveat: pool size

Concurrency is capped by `DB_MAX_CONNECTIONS` (default `5` in `.env.example`).
With 7 futures and a pool of 5, two queries wait for a free connection — still
faster than fully sequential, and other requests are not starved for long since
every query is a cheap aggregate. Using the single-round-trip scalar query above
brings it down to 5 concurrent queries, which fits the default pool exactly.

## Expected latency

| Mode | Latency (≈ RTT = network round-trip + query time) |
|------|------------------------------|
| Sequential (today) | 7 × RTT |
| `try_join!`, pool ≥ 7 | ≈ 1 × max(RTT) |
| `try_join!`, pool = 5 | ≈ 2 × RTT |

Against a managed Postgres with ~20 ms RTT, that's roughly 140 ms → 20–40 ms.
Measure with `tests/admin_overview.rs` plus `hyperfine` against `GET /admin/overview`
before and after.

## Error handling

`try_join!` returns the first `sqlx::Error` and drops the remaining futures, so the
handler's existing `?`/500 mapping is unchanged. The existing
`tests/admin_overview.rs` covers the response shape and needs no changes.
