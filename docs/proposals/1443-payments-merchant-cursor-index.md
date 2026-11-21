# Proposal: composite index for `payments_by_merchant_cursor` (#1443)

## Problem
`payments_by_merchant_cursor()` in `src/services/payments.rs` runs:

```sql
WHERE merchant_id = $1 AND (created_at, id) < ($2, $3)
ORDER BY created_at DESC, id DESC
LIMIT $6
```

Without an index matching the keyset, Postgres scans every row for the
merchant and sorts before applying `LIMIT` — O(n) per page.

## Proposal
A maintainer adds a migration (after `0031_payments_analytics_index.sql`):

```sql
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_transactions_merchant_created_id
    ON transactions (merchant_id, created_at DESC, id DESC);
```

- `CONCURRENTLY` avoids locking writes (the migration must not run inside a transaction).
- The row-value predicate and `ORDER BY` match the index order, so the planner
  does an Index Scan that stops after `LIMIT` rows: O(log n + limit).
- If an existing `(merchant_id, created_at)` index is made redundant, drop it in a follow-up.

## Verification
See `docs/benchmarks/1443-payments-merchant-cursor.md`. A seeded test (10,000
rows, page under threshold) should accompany the migration.
