# Benchmark: `payments_by_merchant_cursor` query plan (#1443)

Run against a merchant with 100k seeded rows:

```sql
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM transactions
WHERE merchant_id = $1 AND (created_at, id) < ($2, $3)
ORDER BY created_at DESC, id DESC
LIMIT 50;
```

## Before (no composite index) — expected shape
```
Limit
  -> Sort  (Sort Key: created_at DESC, id DESC)  top-N heapsort
       -> Bitmap Heap Scan / Seq Scan on transactions
            Filter: merchant_id = $1 AND ROW(created_at, id) < ROW($2, $3)
```
Every matching row is read and sorted; cost grows linearly with merchant volume.

## After `(merchant_id, created_at DESC, id DESC)` — expected shape
```
Limit
  -> Index Scan using idx_transactions_merchant_created_id on transactions
       Index Cond: merchant_id = $1 AND ROW(created_at, id) < ROW($2, $3)
```
No sort node; only `LIMIT` index entries are read regardless of merchant size.

## Recording results
Paste actual `EXPLAIN ANALYZE` output (execution time, buffers) for both runs
here when the maintainer migration lands.
