# Proposal: expose `payment_type` on `GET /transactions` (#1464)

## Problem

`src/services/payments.rs → record_sweep()` inserts rows with `payment_type = 'sweep'`. Customer deposits are inserted without an explicit `payment_type`, so the column defaults to `'deposit'` (or remains null, depending on the migration). Neither value is returned by `GET /transactions`.

A merchant viewing their transaction history cannot tell whether a row represents a customer deposit or an internal sweep to the settlement wallet. That makes reconciliation confusing: both rows carry an `amount_stroops` and a `status`, but there is no visible signal of their purpose.

## Proposed change

### 1. `PAYMENT_COLUMNS` constant (`src/services/payments.rs`)

Add `payment_type` to the `PAYMENT_COLUMNS` constant so every query that uses it fetches the column:

```rust
const PAYMENT_COLUMNS: &str =
    "id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, \
     network, status, confirmations, created_at, updated_at, flagged, payment_type";
```

### 2. `Payment` model (`src/models/payment.rs`)

Add an `Option<String>` field (nullable because existing rows pre-dating the migration have no value) and include it in the CSV export:

```rust
pub struct Payment {
    // … existing fields …
    pub payment_type: Option<String>,
}
```

`CSV_HEADER` and `to_csv_record` gain `payment_type` as the final column so CSV exports also carry the distinction.

### 3. Database migration

```sql
-- migrations/0040_payment_type.sql
ALTER TABLE payments
    ADD COLUMN IF NOT EXISTS payment_type TEXT
        NOT NULL DEFAULT 'deposit'
        CHECK (payment_type IN ('deposit', 'sweep'));

-- Back-fill sweep rows that were inserted before this column existed.
-- record_sweep() already hard-codes 'sweep' in its INSERT; this covers
-- any row that was inserted before the column had a default.
UPDATE payments SET payment_type = 'sweep'
 WHERE payment_type = 'deposit'
   AND tx_hash IN (
       SELECT tx_hash FROM payments
        WHERE status = 'confirmed'
          AND wallet_address = current_setting('app.settlement_wallet_address', true)
   );
```

> The back-fill query is a best-effort heuristic. In practice, sweep rows are identifiable by `status = 'confirmed'` inserted by `record_sweep()`; teams may choose to skip the back-fill and simply expose `null` for older rows.

### 4. No change to `src/api/transactions.rs`

The `list` handler returns whatever the `Payment` struct serialises to. Once the struct carries `payment_type`, the JSON response gains the field automatically via `serde::Serialize`. No handler code changes are needed.

## Response shape (after change)

```json
{
  "data": [
    {
      "id": "f03857f3-…",
      "payment_type": "deposit",
      "status": "confirmed",
      …
    },
    {
      "id": "a1b2c3d4-…",
      "payment_type": "sweep",
      "status": "confirmed",
      …
    }
  ],
  "next_cursor": null
}
```

`payment_type` is `"deposit"` for normal customer payments and `"sweep"` for internal settlement transfers.

## Test plan

A new integration test (`tests/transaction_payment_type.rs`) creates one deposit row and one sweep row for the same merchant, calls `GET /transactions`, and asserts:

- both rows are present;
- the deposit row carries `"payment_type": "deposit"`;
- the sweep row carries `"payment_type": "sweep"`.

See the test file for the complete implementation.

## Alternatives considered

**Filter `sweep` rows out of `GET /transactions` entirely.** Simpler for merchants, but loses audit visibility. Sweeps are real on-chain movements that belong in the history.

**Separate `GET /sweeps` endpoint.** More REST-ful, but adds surface area; a single field on the existing endpoint is cheaper and sufficient for reconciliation.

## Risks

- Merchants who parse the response with strict unknown-field rejection will start receiving `payment_type`; that is a backwards-compatible addition in JSON but could break strict deserializers. This is documented in the API changelog.
- The `Option<String>` on the Rust struct means rows inserted before the migration (no default set) serialise as `null`. After the migration lands with `DEFAULT 'deposit'`, all new rows have a value.
