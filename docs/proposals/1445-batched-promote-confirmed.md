# Proposal: batched `promote_confirmed` (#1445)

## Problem
`promote_confirmed()` fetches ready payments then opens one transaction per payment. A burst of 100 deposits means 100 serial round-trips per poll pass.

## Proposal
Promote all ready payments and credit balances in a single statement:
```sql
WITH promoted AS (
    UPDATE payments SET status = 'confirmed', updated_at = now()
    WHERE status = 'verified' AND confirmations >= $1
    RETURNING user_id, asset, amount
), totals AS (
    SELECT user_id, asset, SUM(amount) AS amount FROM promoted GROUP BY user_id, asset
)
UPDATE balances b SET amount = b.amount + t.amount
FROM totals t WHERE b.user_id = t.user_id AND b.asset = t.asset;
```

## Trade-off
- **Faster:** one transaction and one round-trip regardless of batch size; less lock churn.
- **All-or-nothing:** a single row failure (constraint violation, missing balance row) rolls back every promotion in the batch, delaying all deposits until the bad row is fixed. Mitigations: cap batch size (`LIMIT` via `id IN (SELECT ... LIMIT n)`), and fall back to the per-row path when the batch fails so one bad row cannot stall the rest.

See `docs/benchmarks/1445-promote-confirmed.md`.
