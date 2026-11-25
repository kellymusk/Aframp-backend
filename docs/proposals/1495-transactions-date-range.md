# Proposal #1495: Date-Range Filtering for `GET /transactions`

**Status:** Draft
**Category:** API – Feature
**Priority:** Medium
**Target Branch:** `dev-backend`
**Author:** (proposal author)
**Related:** `src/api/transactions.rs`, `src/services/payments.rs` → `payments_by_merchant_range()`, `tests/transaction_filters.rs`

---

## Summary

Add optional `from` and `to` date-range query parameters to the `GET /transactions` list endpoint. This lets merchants fetch period-specific transactions (e.g. "all deposits in March 2025") directly from the list endpoint, without downloading a CSV export or paging through the entire transaction history.

The filtering semantics reuse the existing `payments_by_merchant_range()` behavior already used by the CSV export path, so the list endpoint and CSV export stay consistent.

---

## Motivation

Today `GET /transactions` uses `payments_by_merchant_cursor()`, which only supports cursor pagination. To reconcile a specific period, a merchant must either:

1. Use the CSV export path (which already supports `from`/`to` via `payments_by_merchant_range()`), or
2. Page through all transactions and filter client-side.

Both are inconvenient for interactive reconciliation. Adding `from`/`to` to the list endpoint makes period-specific queries a first-class capability while keeping the existing cursor pagination intact.

---

## Proposed API

### Query parameters (new)

| Name   | Type   | Required | Description |
|--------|--------|----------|-------------|
| `from` | string | No       | Inclusive lower bound on the transaction date. |
| `to`   | string | No       | Inclusive upper bound on the transaction date. |

Both parameters are optional and independent:

- `from` only → all transactions on or after `from`.
- `to` only → all transactions on or before `to`.
- Both → transactions within `[from, to]` inclusive.
- Neither → current behavior (unfiltered, cursor-paginated).

### Example

```
GET /transactions?from=2025-03-01&to=2025-03-31
```

Returns only transactions dated in March 2025 (inclusive of both endpoints).

---

## Validation Rules

1. **Accepted formats**
   - `YYYY-MM-DD` (date only), interpreted as UTC. `from` is the start of that day (`00:00:00Z`); `to` is the end of that day (`23:59:59Z` / inclusive).
   - RFC3339 timestamp (e.g. `2025-03-01T00:00:00Z`), used as-is.
   - Any other format → `400 Bad Request` with a descriptive error message.

2. **Ordering**
   - If both `from` and `to` are supplied and `from > to`, return `400 Bad Request`.

3. **Range bounds**
   - `from` and `to` are inclusive on both ends.

4. **Empty results**
   - A valid range with no matching transactions returns `200 OK` with an empty `data` array (not `404`).

---

## Interaction with Existing Behavior

### Cursor pagination

- Date-range filtering composes with the existing cursor pagination. The cursor continues to encode the position within the filtered result set.
- The response envelope (`data`, `next_cursor`, etc.) is unchanged.
- Clients that supply `from`/`to` should still follow `next_cursor` until it is `null` to retrieve all matching pages.

### Asset / status filters

- `from`/`to` combine with existing `asset` and `status` filters using logical AND. For example, `GET /transactions?from=2025-03-01&to=2025-03-31&status=completed&asset=USDC` returns completed USDC transactions in March 2025.

### Consistency with CSV export

- The list endpoint must apply the same date-boundary semantics as `payments_by_merchant_range()` (inclusive `from`/`to`, UTC day boundaries for date-only inputs) so that a list query and a CSV export for the same range return the same set of transactions.

---

## Implementation Notes (for maintainer)

> **Note:** Source changes must be made by a maintainer.

- **`src/api/transactions.rs`**
  - Parse and validate `from`/`to` query parameters (formats and ordering per the rules above).
  - When either is present, route the query through a range-aware service call rather than the plain cursor path.
- **`src/services/payments.rs`**
  - Reuse `payments_by_merchant_range()` semantics. If cursor pagination is required over a range, add a range-aware cursor variant (e.g. `payments_by_merchant_range_cursor()`) or extend the existing function to accept a cursor, keeping the date-boundary logic in one place.
  - Avoid duplicating boundary normalization between the CSV export and the list endpoint.
- **`tests/transaction_filters.rs`** (new)
  - Create deposits on different dates.
  - Assert `GET /transactions?from=2025-03-01&to=2025-03-31` returns only March deposits.
  - Cover: `from`-only, `to`-only, invalid format (`400`), `from > to` (`400`), empty range (`200` + empty `data`), and composition with `status`/`asset` filters and cursor pagination.

---

## Documentation Updates

- **`API.md`**: Document the new `from`/`to` query parameters on `GET /transactions`, including formats, inclusivity, validation errors, and interaction with pagination and other filters.
- **`openapi.yaml`**: Add `from` and `to` as optional query parameters (type `string`, `format: date` or `date-time`) on the `GET /transactions` operation, with descriptions and examples.

---

## Backward Compatibility

- Both parameters are optional; omitting them preserves current behavior exactly.
- No response schema changes.
- No breaking changes to existing clients.

---

## Open Questions

1. Should date-only `to` be inclusive of the entire day (proposed: yes) or treated as midnight? Proposal assumes inclusive end-of-day for intuitive "March 2025" queries.
2. Should a maximum range span be enforced (e.g. to bound query cost)? Deferred; can be added later without breaking the API.
3. Timezone: date-only inputs are interpreted as UTC. Confirm this matches merchant expectations or document explicitly.

---

## Tasks

- [x] Write this proposal in `docs/proposals/`.
- [ ] Add `tests/transaction_filters.rs` (maintainer).
- [ ] Update `API.md` and `openapi.yaml`.
- [ ] Implement `from`/`to` in `src/api/transactions.rs` and `src/services/payments.rs` (maintainer).