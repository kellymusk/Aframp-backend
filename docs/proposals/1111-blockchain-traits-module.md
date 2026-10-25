# Refactor: move BlockchainListener trait to a dedicated traits.rs file

**Issue:** [#1111](https://github.com/kellymusk/Aframp-backend/issues/1111)  
**Component:** `src/blockchain/` — module structure  
**Priority:** Low  
**Estimated effort:** ~1 hour

---

## Motivation

Currently `src/blockchain/stellar.rs` contains three distinct concerns:

1. The `BlockchainListener` trait (the public interface)
2. `StellarListener` (the Horizon HTTP implementation)
3. All Horizon JSON parsing (`OperationRecord`, `PaymentsPage`, etc.)

This is in contrast to the payments module, which already follows a cleaner
pattern:

```
src/payments/
├── mod.rs       ← PaymentProvider trait
├── paystack.rs  ← PaystackProvider (real)
└── mock.rs      ← MockProvider (for tests)
```

Mirroring that structure for blockchain makes it straightforward to add
alternative listeners (e.g. a cNGN chain listener, a TON network listener)
and to write tests against the trait without depending on HTTP.

---

## Target structure

```
src/blockchain/
├── mod.rs          ← re-export BlockchainListener + DetectedDeposit (already has pub mods)
├── traits.rs       ← NEW: BlockchainListener trait + DetectedDeposit struct
├── stellar.rs      ← StellarListener only (Horizon HTTP impl)
├── mock.rs         ← NEW: MockBlockchainListener for tests
├── keypair.rs      ← unchanged
├── wallet_crypto.rs← unchanged
└── worker.rs       ← unchanged
```

---

## Required changes (all in protected `src/`)

### 1. Create `src/blockchain/traits.rs`

Extract `DetectedDeposit` and `BlockchainListener` out of `stellar.rs` into a
new `traits.rs`:

```rust
// src/blockchain/traits.rs
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct DetectedDeposit {
    pub tx_hash: String,
    pub destination: String,
    pub amount_stroops: i64,
    pub asset: String,
    pub confirmations: i32,
    pub memo: Option<String>,
}

#[async_trait]
pub trait BlockchainListener: Send + Sync {
    async fn fetch_deposits(&self, addresses: &[String]) -> Result<Vec<DetectedDeposit>, String>;
}
```

### 2. Create `src/blockchain/mock.rs`

Following the same pattern as `src/payments/mock.rs`:

```rust
// src/blockchain/mock.rs
use async_trait::async_trait;
use crate::blockchain::traits::{BlockchainListener, DetectedDeposit};

/// A no-op listener for unit/integration tests.
/// Returns an empty deposit list by default; can be extended with
/// test-controlled fixtures as needed.
pub struct MockBlockchainListener;

#[async_trait]
impl BlockchainListener for MockBlockchainListener {
    async fn fetch_deposits(&self, _addresses: &[String]) -> Result<Vec<DetectedDeposit>, String> {
        Ok(vec![])
    }
}
```

### 3. Update `src/blockchain/stellar.rs`

- Remove `DetectedDeposit` and `BlockchainListener` definitions.
- Add `use crate::blockchain::traits::{BlockchainListener, DetectedDeposit};`
- No logic changes.

### 4. Update `src/blockchain/mod.rs`

```rust
pub mod keypair;
pub mod mock;       // add
pub mod stellar;
pub mod traits;     // add
pub mod wallet_crypto;
pub mod worker;

// Re-export the public surface
pub use traits::{BlockchainListener, DetectedDeposit};
```

---

## Acceptance criteria

- `BlockchainListener` and `DetectedDeposit` are defined in `traits.rs`, not `stellar.rs`
- `mock.rs` exists with a `MockBlockchainListener` that implements `BlockchainListener`
- All existing tests pass unchanged (`cargo test`)
- A new test in `tests/blockchain_mock.rs` exercises `MockBlockchainListener` via the trait

The test file for that last criterion is provided in this PR at
`tests/blockchain_mock.rs`.

---

## Why this matters

- Lets `tests/` use `MockBlockchainListener` without importing the Horizon HTTP
  implementation.
- Makes it trivial to stub deposit behaviour in integration tests (return a
  known set of deposits without hitting Horizon).
- Mirrors the established `PaymentProvider` / `MockProvider` pattern already
  in the codebase.
