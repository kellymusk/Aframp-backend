# Benchmark: `age_verified_deposits` at 1M rows (#1444)

Dataset: 1,000,000 `payments` rows, 200 with `status = 'verified'`. Reproduce with `EXPLAIN (ANALYZE, BUFFERS)` on the UPDATE (inside `BEGIN; ... ROLLBACK;`).

| Plan | Scan | Approx. time |
|------|------|--------------|
| No index | `Seq Scan on payments` (Filter: status = 'verified', Rows Removed ~999,800) | ~180 ms |
| `payments_verified_idx` | `Index Scan using payments_verified_idx` | ~0.5 ms |

Figures are indicative from a local Postgres 16 run; re-measure on production-like hardware before relying on exact numbers.
