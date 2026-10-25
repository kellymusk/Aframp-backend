# Enhancement: supported assets and zero entries in `GET /balance`

**Issue:** [#1247](https://github.com/kellymusk/Aframp-backend/issues/1247)
**Component:** `src/api/balances.rs`, `src/services/balances.rs`

## Problem

`GET /balance` returns `Vec<Balance>`. A merchant with no deposits gets `[]`,
with no hint of which assets are supported.

## Proposed change

Wrap the response in an envelope and zero-fill every supported asset:

```rust
pub const SUPPORTED_ASSETS: &[&str] = &["XLM", "cNGN"];

#[derive(Serialize)]
pub struct BalanceResponse {
    pub supported_assets: &'static [&'static str],
    pub balances: Vec<Balance>, // one per SUPPORTED_ASSETS entry, in that order
}
```

In the handler, start from the DB rows and push a zero `Balance`
(`available: 0, pending: 0, updated_at: None`) for each supported asset with
no row. `updated_at` becomes `Option<DateTime<Utc>>` in the response.

This is a breaking change to the response shape (array → object); ship it with
a CHANGELOG entry. The shape is documented in `API.md` and as
`BalanceEnvelope` in `openapi.yaml`.

## Tests

`tests/balance_flow.rs` currently pins `[]` for a new merchant. When the change
lands, replace that assertion with the one sketched in
`new_merchant_balance_lists_supported_assets` (ignored until then): two
entries, both zero, plus `supported_assets == ["XLM", "cNGN"]`.

Totals / USD–NGN equivalents are covered separately by
[1112-multi-currency-balance-fiat.md](1112-multi-currency-balance-fiat.md).
