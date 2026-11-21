# Proposal: partial index for `age_verified_deposits` (#1444)

## Problem
`age_verified_deposits()` runs `UPDATE payments SET confirmations = confirmations + 1 WHERE status = 'verified'` every poll cycle (default 60s). Without an index on that predicate Postgres sequentially scans the whole `payments` table, which is overwhelmingly `confirmed` rows at scale.

## Proposal
Add a migration:
```sql
CREATE INDEX CONCURRENTLY IF NOT EXISTS payments_verified_idx
    ON payments (id) WHERE status = 'verified';
```
The index only contains rows currently awaiting confirmations, so it stays tiny; rows leave it once promoted to `confirmed`.

## Trade-offs
- Small write cost on transitions into/out of `verified`.
- `CONCURRENTLY` cannot run inside a transaction; the migration must be non-transactional.

See `docs/benchmarks/1444-verified-index.md` for `EXPLAIN ANALYZE` figures.
