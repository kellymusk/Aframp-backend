# Proposal: `GET /payment-requests?asset=` filter (#1460)

## Problem

`GET /payment-requests` supports filtering by `status` but not by `asset`. A
merchant who accepts both XLM and cNGN cannot list only their cNGN payment
requests without fetching every page and filtering client-side. With pagination
this means multiple round-trips and unbounded client-side memory use as the
catalogue grows.

## Proposed change

Add an optional `asset` query parameter to `GET /payment-requests`.

```
GET /payment-requests?asset=cNGN
GET /payment-requests?asset=XLM&status=pending
```

### API contract

| Parameter | Type   | Required | Default | Validation |
|-----------|--------|----------|---------|------------|
| `asset`   | string | no       | (none)  | No server-side enum — values are stored verbatim, so a typo like `asset=XLMM` returns an empty list rather than a `400`. This matches how `status` is validated (rejected) versus how `cursor` is validated (rejected); for asset, silent-empty is the right trade-off because the set of valid assets is open-ended. |

When `asset` is absent the behaviour is unchanged (all assets returned).

### Service layer

`payment_requests_by_merchant_cursor` in `src/services/payment_requests.rs`
already joins the `wallets` table via `pr.wallet_id = w.id`. The asset is
stored directly on `payment_requests.asset` (not on the wallet), so the filter
is a single additional `AND` clause:

```sql
AND ($7::text IS NULL OR pr.asset = $7)
```

`$7` is bound as `Option<&str>`:
- `None` → `$7::text IS NULL` is `TRUE`, the clause is a no-op.
- `Some("cNGN")` → filters to cNGN rows only.

No schema migration, no new index (the existing `(merchant_id, created_at DESC)`
index already drives this query; asset selectivity is high enough that adding
it to the index is premature until query plans show a need).

### Handler layer

`ListParams` in `src/api/payment_requests.rs` gains one field:

```rust
pub asset: Option<String>,
```

The value is passed through to the service call without normalisation. An empty
string (`?asset=`) is treated the same as absent because `""` will match no
row (all asset values are non-empty).

### Interaction with other filters

`asset` composes freely with `status`, `include_cancelled`, `limit`, and
`cursor`. All combinations are valid; the `AND` clauses stack.

## Alternatives considered

**Validate against a known enum (`XLM`, `cNGN`, …)**
Rejected: the asset list is open-ended and stored verbatim in the DB. A strict
enum would need updating every time a new asset is introduced and would return
`400` for valid historical assets the server doesn't know about yet. Silent-empty
is safer.

**Filter on the wallet's asset column instead of `payment_requests.asset`**
Not applicable: `payment_requests.asset` is the authoritative field. Wallets
don't have an asset column; the payment request owns the asset choice.

## Tasks for a maintainer

1. Add `asset: Option<String>` to `ListParams` in
   `src/api/payment_requests.rs`.
2. Pass it to `payment_requests_by_merchant_cursor` in the `list()` handler.
3. Add `asset: Option<&str>` parameter to `payment_requests_by_merchant_cursor`
   in `src/services/payment_requests.rs` and extend the SQL with:
   `AND ($7::text IS NULL OR pr.asset = $7)`
4. Bump the bind count (`$6` → `$7` for the existing `limit + 1` bind).

## Tests

`tests/payment_request_flow.rs` — new test `payment_request_list_filters_by_asset`:

1. Create one XLM and one cNGN payment request under the same merchant.
2. `GET /payment-requests?asset=cNGN` → only the cNGN request is returned.
3. `GET /payment-requests?asset=XLM` → only the XLM request is returned.
4. `GET /payment-requests` (no filter) → both requests are returned.
5. `GET /payment-requests?asset=USDC` (unknown asset) → empty list, `200`.

## References

- `src/api/payment_requests.rs` → `list()`, `ListParams`
- `src/services/payment_requests.rs` → `payment_requests_by_merchant_cursor`
- Related: #1268 (payment-request view refactor), #1446 (cheap ETag)
