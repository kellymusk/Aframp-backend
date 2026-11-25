# Proposal #1497: Add `last_deposit_at` to `GET /balance`

**Category:** API – UX
**Priority:** Low
**Estimated Time:** 1 hour
**Target Branch:** `dev-backend`

## Summary

Add a `last_deposit_at` field to each asset entry in the `GET /balance` response. The field reports the timestamp of the most recent confirmed deposit received for that asset, eliminating the need for a separate `GET /transactions?limit=1` call per asset when a merchant dashboard wants to display "last deposit received".

## Motivation

The balance response currently returns `available`, `pending`, and `updated_at` per asset. `updated_at` reflects the last time the balance row was touched by *any* operation (including withdrawals), so it cannot be used to answer "when did we last receive a deposit?". Merchants currently have to issue an extra `GET /transactions?limit=1` request per asset to find this out. Adding `last_deposit_at` to the balance response removes that extra round-trip.

## Definition

`last_deposit_at` is defined as:

> The `MAX(created_at)` over all **confirmed** payments (status `confirmed`) for the merchant and the given asset.

- It is `null` when the merchant has no confirmed deposits for that asset.
- It is independent of `updated_at`, which continues to reflect any balance mutation.
- Only payments with status `confirmed` are considered; pending or failed payments do not affect the value.

## API Changes

### Response field placement

The new field is added to each `Balance` row in the `GET /balance` response, alongside the existing `available`, `pending`, and `updated_at` fields:

```json
{
  "balances": [
    {
      "asset": "USDC",
      "available": "100.0000000",
      "pending": "0.0000000",
      "updated_at": "2025-01-15T10:30:00Z",
      "last_deposit_at": "2025-01-14T09:12:00Z"
    },
    {
      "asset": "XLM",
      "available": "0.0000000",
      "pending": "0.0000000",
      "updated_at": "2025-01-10T08:00:00Z",
      "last_deposit_at": null
    }
  ]
}
```

- Type: `string` (RFC 3339 timestamp) or `null`.
- Field name: `last_deposit_at`.
- Placement: per-asset `Balance` row (not a top-level field).

## Implementation Notes (for a maintainer)

Source changes must be made by a maintainer. The relevant files are:

- `src/api/balances.rs` — the `GET /balance` handler; the response serialization must include the new field.
- `src/services/balances.rs` — the balance query/aggregation; the new `last_deposit_at` value is computed here.
- `src/models/balance.rs` — the `Balance` model; add an optional `last_deposit_at` field (e.g. `Option<DateTime<Utc>>`).

### SQL aggregation

Compute `last_deposit_at` per asset with an aggregation over the `payments` table, restricted to confirmed payments for the merchant. Either of the following approaches is acceptable:

**Option A — correlated subquery:**

```sql
SELECT
    b.asset,
    b.available,
    b.pending,
    b.updated_at,
    (
        SELECT MAX(p.created_at)
        FROM payments p
        WHERE p.merchant_id = b.merchant_id
          AND p.asset = b.asset
          AND p.status = 'confirmed'
    ) AS last_deposit_at
FROM balances b
WHERE b.merchant_id = $1;
```

**Option B — LEFT JOIN LATERAL:**

```sql
SELECT
    b.asset,
    b.available,
    b.pending,
    b.updated_at,
    d.last_deposit_at
FROM balances b
LEFT JOIN LATERAL (
    SELECT MAX(p.created_at) AS last_deposit_at
    FROM payments p
    WHERE p.merchant_id = b.merchant_id
      AND p.asset = b.asset
      AND p.status = 'confirmed'
) d ON TRUE
WHERE b.merchant_id = $1;
```

Both approaches yield `NULL` when there are no confirmed deposits, which maps to `null` in the JSON response. An index on `payments (merchant_id, asset, status, created_at)` is recommended to keep the aggregation cheap.

## Testing

Add `tests/balance_flow.rs` covering:

- After a deposit is confirmed, `GET /balance` includes `last_deposit_at` matching the deposit timestamp.
- Assets with no confirmed deposits return `last_deposit_at: null`.
- Pending/failed payments do not update `last_deposit_at`.

## Documentation

- Update `API.md` to document the new `last_deposit_at` field on the `GET /balance` response.
- Update `openapi.yaml` to add `last_deposit_at` (nullable string, `format: date-time`) to the `Balance` schema.

## Backward Compatibility

This is an additive change. Existing clients that do not read `last_deposit_at` are unaffected. The field is nullable, so no existing field semantics change.

## References

- `src/api/balances.rs`
- `src/services/balances.rs`
- `src/models/balance.rs`