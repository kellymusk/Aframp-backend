# Proposal: Per-Asset Daily Deposit Summary for `GET /analytics/payments`

**Issue:** #1494
**Category:** API – Feature
**Target Branch:** `dev-backend`

## Summary

Add a `summary` field to the `GET /analytics/payments` response that provides a per-asset daily breakdown of confirmed deposits. This allows merchants building settlement dashboards to consume aggregated per-asset totals directly, rather than aggregating across the returned rows client-side.

## Motivation

Currently `GET /analytics/payments` returns the raw daily confirmed totals produced by `daily_confirmed_totals()` in `src/services/payments.rs` (surfaced via `src/api/analytics.rs`). These rows are aggregated by day and asset, but the endpoint does not include a per-asset daily breakdown summary (e.g. "XLM: 500 deposits this week, cNGN: 20 deposits this week"). Merchants must aggregate across the returned rows themselves, which is error-prone and duplicates logic across clients.

Adding a `summary` field centralizes this aggregation in the API and gives clients a stable, documented shape to consume.

## Proposed Change

### Response shape

The `GET /analytics/payments` response gains a new top-level `summary` field. The existing fields are unchanged.

```json
{
  "data": [ /* existing per-day rows, unchanged */ ],
  "summary": [
    {
      "asset": "XLM",
      "days": [
        { "date": "2024-06-01", "total": "500.0000000", "count": 12 },
        { "date": "2024-06-02", "total": "250.0000000", "count": 7 }
      ]
    },
    {
      "asset": "cNGN",
      "days": [
        { "date": "2024-06-01", "total": "20.0000000", "count": 3 }
      ]
    }
  ]
}
```

### Field semantics

- `summary` is an array of per-asset objects.
- Each entry has:
  - `asset` (string): the asset code (e.g. `XLM`, `cNGN`).
  - `days` (array): per-day totals for that asset, ordered by `date` ascending.
    - `date` (string, `YYYY-MM-DD`): the calendar day.
    - `total` (string): sum of confirmed deposit amounts for that asset on that day, serialized as a decimal string to avoid floating-point precision loss.
    - `count` (integer): number of confirmed deposits for that asset on that day.
- Only days with at least one confirmed deposit for the asset are included (no zero-filled days).
- Assets with no confirmed deposits in the requested range are omitted.
- The `summary` is derived from the same underlying data as `daily_confirmed_totals()`; it is a re-grouping of the existing rows by asset, then by day.

### Empty-range behaviour

When the requested date range contains no confirmed deposits (or the range is empty), `summary` MUST be an empty array (`[]`), never `null`. This matches the existing convention for `data` and keeps client parsing simple.

## Implementation Notes

- `src/services/payments.rs` → `daily_confirmed_totals()` already aggregates confirmed payments by day and asset. The `summary` can be built by grouping the returned rows by `asset` and collecting the per-day entries, preserving ascending date order.
- `src/api/analytics.rs` → serialize the new `summary` field on the analytics response struct. Ensure the field is always present and defaults to an empty array when there is no data (e.g. `#[serde(default)]` plus explicit empty-vec initialization, or a `skip_serializing_if` avoided so the key is always emitted).
- No database schema changes are required.

## Tests

Add `tests/analytics_payments_flow.rs` covering:

1. **Per-asset per-day totals:** create deposits across two assets (e.g. `XLM` and `cNGN`) over three days, call `GET /analytics/payments`, and assert that `summary` contains the correct per-asset per-day `total` and `count` values.
2. **Empty date range:** request a range with no deposits and assert that `summary` is an empty array (`[]`), not `null`.

## Documentation Updates

- **`API.md`:** document the new `summary` field on `GET /analytics/payments`, including the shape, field semantics, ordering, and empty-range behaviour described above.
- **`openapi.yaml`:** add the `summary` property to the analytics payments response schema, defining the per-asset object (`asset`, `days`) and the per-day object (`date`, `total`, `count`), and mark it as required with an array type (empty array when no data).

## Backwards Compatibility

This is an additive change. Existing fields are unchanged, and clients that ignore the new `summary` field continue to work. The only observable difference is the presence of an additional top-level key.

## Out of Scope

- Pagination or filtering of the `summary` array.
- Aggregation windows other than the requested date range.
- Changes to the underlying `daily_confirmed_totals()` aggregation logic.