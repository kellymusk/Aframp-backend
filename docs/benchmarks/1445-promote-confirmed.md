# Benchmark: per-row vs batched `promote_confirmed` (#1445)

Local Postgres 16, ~0.3 ms network RTT. Indicative figures; re-measure before relying on them.

| Ready deposits | Per-row transactions | Batched CTE |
|----------------|----------------------|-------------|
| 50  | ~60 ms  | ~4 ms  |
| 500 | ~620 ms | ~25 ms |

Per-row cost scales linearly with round-trips; the batch is dominated by the single UPDATE's row work.
