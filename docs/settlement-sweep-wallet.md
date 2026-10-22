# Settlement Sweep Wallet — Design & Implementation Plan

**Issue:** [#1276](https://github.com/kellymusk/Aframp-backend/issues/1276)  
**Related source:** `src/blockchain/stellar.rs`, `src/blockchain/worker.rs`,
`src/services/withdrawals.rs`, `src/config.rs`  
**PRD reference:** §9.1 (Settlement & payout pipeline, Stage A)

---

## Current State

`STELLAR_SYSTEM_WALLET_ADDRESS` is validated at startup (see `src/config.rs`)
and documented in `README.md`:

> Settlement/sweep wallet: Each merchant's Stellar secret is held (encrypted)
> by the platform, but nothing yet sweeps funds from individual merchant
> wallets into a platform settlement wallet. `STELLAR_SYSTEM_WALLET_ADDRESS`
> is still validated at startup and reserved for this, but isn't used by
> anything yet.

The encrypted secret key exists for every merchant wallet (AES-256-GCM via
`src/blockchain/wallet_crypto.rs`) and can already be decrypted at runtime.
The pieces needed for sweep are all present; they just aren't wired together.

---

## Why This Matters

Without sweep, the Stage A → Stage B payout pipeline is broken:

```
merchant wallet (Stellar)
         │
         │  ← sweep (MISSING)
         ▼
platform settlement wallet (STELLAR_SYSTEM_WALLET_ADDRESS)
         │
         │  ← cNGN redemption (future)
         ▼
Aframp's NGN bank account (Paystack balance)
         │
         │  ← Paystack Transfers (wired, tested)
         ▼
merchant's Nigerian bank account
```

Until sweep is implemented, `POST /withdraw` can atomically debit the
merchant's *Aframp ledger* balance but the matching crypto never moves, so
Aframp's Paystack balance stays at ₦0 and all real-money Paystack transfers
fail with "Your balance is not enough to fulfil this request."

---

## Proposed Implementation (requires `src/` write access)

### 1. New migration — `sweep_transactions` table

```sql
-- migrations/0009_sweep_transactions.sql
CREATE TABLE sweep_transactions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_id     UUID NOT NULL REFERENCES merchants(id),
    wallet_id       UUID NOT NULL REFERENCES wallets(id),
    withdrawal_id   UUID REFERENCES withdrawals(id),
    amount_stroops  BIGINT NOT NULL,
    asset           TEXT NOT NULL DEFAULT 'XLM',
    stellar_tx_hash TEXT,
    status          TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'submitted', 'confirmed', 'failed')),
    failure_reason  TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ON sweep_transactions (merchant_id);
CREATE INDEX ON sweep_transactions (withdrawal_id);
CREATE INDEX ON sweep_transactions (status);
```

> **Note:** Adding this migration requires `migrations/` write access, which
> contributors do not have. The migration is documented here so the maintainer
> can apply it directly.

### 2. New service — `src/services/sweep.rs`

Core logic:

```rust
use crate::blockchain::wallet_crypto;
use crate::models::Wallet;
use sqlx::PgPool;
use uuid::Uuid;

/// Decrypts the merchant wallet's secret key, builds a Stellar payment
/// transaction moving `amount_stroops` of `asset` to the platform settlement
/// wallet, and submits it to Horizon.
///
/// Returns the Stellar transaction hash on success.
pub async fn sweep_to_platform(
    db: &PgPool,
    horizon_url: &str,
    wallet: &Wallet,
    encryption_key: &[u8; 32],
    amount_stroops: i64,
    asset: &str,
    withdrawal_id: Option<Uuid>,
    system_wallet_address: &str,
) -> Result<String, SweepError> {
    // 1. Decrypt the merchant's Stellar secret key.
    let secret_key = wallet_crypto::decrypt_secret(
        &wallet.encrypted_secret_key,
        encryption_key,
    ).map_err(SweepError::Decrypt)?;

    // 2. Build a Stellar payment transaction (XDR).
    //    Uses stellar-base or stellar-sdk to construct:
    //      - source: wallet.address (merchant wallet)
    //      - destination: system_wallet_address
    //      - amount: amount_stroops / 10_000_000 (Stellar decimal)
    //      - asset: native (XLM) or credit asset
    //      - fee: fetched from Horizon /fee_stats or hardcoded 100 stroops
    //      - sequence: fetched from Horizon /accounts/{address}
    let tx_xdr = build_payment_tx(
        &wallet.address,
        &secret_key,
        system_wallet_address,
        amount_stroops,
        asset,
        horizon_url,
    ).await.map_err(SweepError::Build)?;

    // 3. Submit to Horizon.
    let tx_hash = submit_transaction(horizon_url, &tx_xdr)
        .await
        .map_err(SweepError::Submit)?;

    // 4. Record the sweep in sweep_transactions.
    sqlx::query(
        "INSERT INTO sweep_transactions
             (merchant_id, wallet_id, withdrawal_id, amount_stroops, asset,
              stellar_tx_hash, status)
         VALUES ($1, $2, $3, $4, $5, $6, 'submitted')",
    )
    .bind(wallet.merchant_id)
    .bind(wallet.id)
    .bind(withdrawal_id)
    .bind(amount_stroops)
    .bind(asset)
    .bind(&tx_hash)
    .execute(db)
    .await
    .map_err(SweepError::Database)?;

    Ok(tx_hash)
}

#[derive(Debug, thiserror::Error)]
pub enum SweepError {
    #[error("failed to decrypt wallet secret: {0}")]
    Decrypt(String),
    #[error("failed to build Stellar transaction: {0}")]
    Build(String),
    #[error("Stellar submission failed: {0}")]
    Submit(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}
```

### 3. Trigger sweep from `src/services/withdrawals.rs`

The withdrawal service already calls Paystack. Before that call, add the sweep:

```rust
// In create_withdrawal():

// Step 1: debit available balance (already done)
// Step 2: insert withdrawal record (already done)

// Step 3: Sweep merchant funds to platform wallet (NEW)
let sweep_result = sweep::sweep_to_platform(
    db, &state.horizon_url, &wallet,
    &state.wallet_encryption_key,
    req.amount_stroops, &asset,
    Some(withdrawal.id),
    &state.system_wallet_address,
).await;

match sweep_result {
    Ok(tx_hash) => {
        tracing::info!(
            withdrawal_id = %withdrawal.id,
            tx_hash = %tx_hash,
            "swept merchant funds to platform wallet"
        );
    }
    Err(err) => {
        // Step 4: refund on sweep failure
        tracing::error!(
            withdrawal_id = %withdrawal.id,
            error = %err,
            "sweep failed — refunding withdrawal"
        );
        balances::apply_delta(db, &UpdateBalance {
            merchant_id: wallet.merchant_id,
            asset: asset.clone(),
            available_delta: req.amount_stroops,
            pending_delta: 0,
        }).await?;
        set_withdrawal_failed(db, withdrawal.id, &err.to_string()).await?;
        return Err(/* SweepFailed error */);
    }
}

// Step 5: Call Paystack (existing code)
```

### 4. `AppState` additions

```rust
pub struct AppState {
    // … existing fields …
    pub horizon_url: String,            // already in config but not in AppState
    pub system_wallet_address: String,  // add this
}
```

### 5. Worker integration (optional, Phase 2)

For automated sweeps (not tied to a withdrawal trigger), the background worker
can periodically check for wallets with confirmed balances above a threshold
and sweep them:

```rust
// In src/blockchain/worker.rs run():
if should_sweep(&state.db, &wallet).await {
    sweep_to_platform(…).await;
}
```

This decouples the sweep from the withdrawal request but adds complexity.
**Recommendation:** Start with withdrawal-triggered sweeps (simpler, auditable)
before adding background sweeps.

---

## Error Handling

| Failure point | Recovery |
|---|---|
| Decrypt failure | Refund withdrawal, mark `failed`, log key error |
| Sequence fetch failure (Horizon unavailable) | Retry up to 3× with exponential backoff; then refund |
| Transaction build failure | Refund withdrawal, mark `failed` |
| Horizon submission failure (tx invalid) | Refund withdrawal; record `sweep_transactions.status = failed` |
| Horizon submission failure (timeout) | **Do NOT refund yet** — the tx may have landed. Poll `/transactions/{hash}` to confirm before refunding |

The timeout case is the hardest: a Stellar transaction can land even if the
initial HTTP submission request times out. The implementation must:

1. Store the `stellar_tx_hash` before submission.
2. On timeout, poll Horizon's `/transactions/{hash}` for up to 30 seconds.
3. If confirmed: proceed to Paystack call.
4. If still not found after 30 s: refund and mark failed (the transaction will
   eventually expire at its `max_ledger` boundary).

---

## Database Schema

```
sweep_transactions
├── id              UUID (PK)
├── merchant_id     UUID → merchants.id
├── wallet_id       UUID → wallets.id
├── withdrawal_id   UUID → withdrawals.id (nullable: background sweeps have no withdrawal)
├── amount_stroops  BIGINT
├── asset           TEXT ('XLM', 'cNGN', …)
├── stellar_tx_hash TEXT (nullable until submitted)
├── status          TEXT ('pending'|'submitted'|'confirmed'|'failed')
├── failure_reason  TEXT (nullable)
├── created_at      TIMESTAMPTZ
└── updated_at      TIMESTAMPTZ
```

---

## Sequence Diagram

```
POST /withdraw
    │
    ├─ Validate + debit balance
    ├─ Insert withdrawal record (pending)
    │
    ├─ sweep_to_platform()
    │     ├─ Decrypt merchant secret key (AES-256-GCM)
    │     ├─ Fetch account sequence from Horizon
    │     ├─ Build + sign Stellar payment tx
    │     ├─ Submit to Horizon
    │     ├─ Insert sweep_transactions row (submitted)
    │     └─ Return tx_hash
    │
    ├─ (sweep success) → continue to Paystack
    │     └─ PaystackProvider::initiate_transfer()
    │           ├─ success → update withdrawal (completed)
    │           └─ failure → refund balance, update withdrawal (failed)
    │
    └─ (sweep failure) → refund balance, update withdrawal (failed)
                         update sweep_transactions (failed)
```

---

## Stellar Transaction Construction

The Stellar SDK for Rust does not yet have first-class support in the Rust
ecosystem. The two options are:

### Option A: Use `stellar-base` crate

```toml
# Cargo.toml (requires write access)
stellar-base = "0.5"
```

```rust
use stellar_base::transaction::{Transaction, Operation};
use stellar_base::amount::Stroops;

let op = Operation::payment(destination, asset, Stroops::new(amount_stroops));
let tx = Transaction::new(source_keypair, sequence, fee, vec![op]);
let envelope = tx.sign(&secret_keypair, &network_passphrase);
let xdr = envelope.to_xdr_base64();
```

### Option B: Call Horizon's `/transactions` via `reqwest` with hand-built XDR

More verbose but removes the dependency. The Stellar XDR format is documented
at [stellar.org/developers](https://developers.stellar.org/docs/learn/fundamentals/transactions).

**Recommendation:** Option A if the dependency is acceptable; otherwise a
minimal XDR builder targeting only the `payment` operation type is ~200 lines.

---

## Dependency on External Stellar SDK

Because `Cargo.toml` is a protected path, contributors cannot add the
`stellar-base` dependency. The maintainer must add it, or the implementation
must use raw XDR (Option B above) or call out to a helper process.

---

## Tests (`tests/sweep_wallet.rs`)

See `tests/sweep_wallet.rs` (added alongside this doc) for the test plan:

- Mock a successful sweep and assert `sweep_transactions` row is created.
- Mock a sweep failure and assert balance is refunded and withdrawal is `failed`.
- Assert `STELLAR_SYSTEM_WALLET_ADDRESS` is forwarded correctly.
- Integration test: full `POST /withdraw` flow with a mock sweep provider.

---

## README.md and PRD.md Updates

See the changes in this PR to `README.md` (Status table) and `PRD.md` (§8).
Both documents have been updated to reflect:

- The settlement/sweep gap description now references this design document.
- `PRD.md §12` roadmap item 1 has been updated with the implementation plan.

---

## References

- [`src/blockchain/wallet_crypto.rs`](../src/blockchain/wallet_crypto.rs) — `decrypt_secret`
- [`src/services/withdrawals.rs`](../src/services/withdrawals.rs) — withdrawal flow
- [`src/config.rs`](../src/config.rs) — `STELLAR_SYSTEM_WALLET_ADDRESS` parsing
- [Stellar Horizon API docs](https://developers.stellar.org/docs/data/horizon)
- [Stellar XDR reference](https://github.com/stellar/stellar-xdr)
- [Issue #1276](https://github.com/kellymusk/Aframp-backend/issues/1276)
- [PRD.md §9.1](../PRD.md) — Settlement & payout pipeline
