# Fix: path_payment_strict_send should credit `amount_received`, not `amount`

**Issue:** [#1110](https://github.com/kellymusk/Aframp-backend/issues/1110)  
**Component:** `src/blockchain/stellar.rs` — `record_to_deposit` / `OperationRecord`  
**Priority:** Low  
**Estimated effort:** ~1 hour

---

## Problem

Horizon's payments endpoint returns three distinct amount fields for a
`path_payment_strict_send` operation:

| Field            | Meaning                                                      |
|------------------|--------------------------------------------------------------|
| `amount`         | Amount **sent** — the input side of the swap (what left the sender's wallet) |
| `amount_received`| Amount **received** — what actually arrived at the destination |
| `destination_min`| Minimum the sender was willing to deliver (a floor, not the actual value) |

The current `record_to_deposit` in `src/blockchain/stellar.rs` reads `amount`
for all operation types:

```rust
"path_payment_strict_send"
    if record.destination.as_deref() == Some(address) =>
{
    record.amount.as_deref()   // BUG: this is the *sent* amount
}
```

For `path_payment_strict_receive` and plain `payment`, `amount` correctly
represents what arrived.  For `path_payment_strict_send` it does not — the
sender specifies an exact input, and the DEX path determines how much the
recipient gets.

### Concrete example

A merchant wallet receives a cNGN path payment where the sender swaps XLM:

```json
{
  "type": "path_payment_strict_send",
  "destination": "GMERCHANT...",
  "amount": "10.0000000",
  "amount_received": "14500.0000000",
  "asset_type": "native",
  "destination_asset_code": "cNGN"
}
```

Current code credits `10 XLM` worth of stroops (`100_000_000`) to the merchant.  
Correct behaviour: credit `14,500 cNGN` worth of stroops (`145_000_000_000`).

---

## Required changes (both in `src/blockchain/stellar.rs`)

### 1. Add `amount_received` to `OperationRecord`

```rust
#[derive(Debug, Deserialize)]
struct OperationRecord {
    // ... existing fields ...

    /// The actual received amount for `path_payment_strict_send`.
    /// Horizon populates this field; `amount` for strict_send is the *sent* side.
    #[serde(default)]
    amount_received: Option<String>,
}
```

### 2. Update `record_to_deposit` to use `amount_received` for strict_send

```rust
"path_payment_strict_send"
    if record.destination.as_deref() == Some(address) =>
{
    // Use amount_received (what arrived) not amount (what was sent).
    // Fall back to amount only if amount_received is absent, to preserve
    // compatibility with any older Horizon responses that omit the field.
    record.amount_received.as_deref().or(record.amount.as_deref())
}
```

---

## Acceptance criteria

All of the following must pass after the fix:

- `strict_send_uses_amount_received_not_amount` — primary fix
- `strict_send_falls_back_to_amount_when_amount_received_missing` — graceful fallback
- `regular_payment_still_uses_amount_field` — no regression
- `strict_receive_uses_amount_field_unchanged` — no regression
- `strict_send_ignored_for_different_destination` — existing behaviour preserved
- `failed_transaction_produces_no_deposit` — existing behaviour preserved

The specification tests for these cases live in
`tests/path_payment_strict_send_amount.rs`.

---

## Horizon API reference

- [Stellar Horizon: Payments for Account](https://developers.stellar.org/docs/data/horizon/api-reference/resources/operations/object/path-payment-strict-send)
- `amount_received` is documented as _"Amount received by the destination account."_ for strict-send operations.

---

## Notes

- No migration required — this is a runtime parsing fix only.
- No API contract change — `DetectedDeposit.amount_stroops` field name is unchanged.
- The fix is safe to deploy without downtime; at worst a re-detection pass on
  historical strict_send deposits would be needed if any were mis-credited.
