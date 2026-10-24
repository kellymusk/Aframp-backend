# Deposit Polling Worker — Load & Stress Testing

**Issue:** [#1274](https://github.com/kellymusk/Aframp-backend/issues/1274)  
**Related source:** `src/blockchain/worker.rs`, `src/blockchain/stellar.rs`

---

## Overview

The Stellar deposit polling worker is a critical background loop. If it falls
behind, crashes silently, or starves under load, merchants are not credited for
real payments. This document records the test harness design, the expected scale
limits, and the gaps remaining.

---

## Test Harness

`tests/deposit_polling_worker_load.rs` implements the following test suite:

| Test | DB required | Purpose |
|------|-------------|---------|
| `mock_listener_returns_deposit_per_address` | No | Baseline: listener contract |
| `half_error_listener_returns_half_deposits` | No | 50% failure leaves 50% intact |
| `slow_listener_completes_without_deadlock` | No | 50 ms delay doesn't stall |
| `empty_listener_returns_no_deposits` | No | Empty cycle is a no-op |
| `poll_100_addresses_all_deposits_returned` | No | 100-wallet scale smoke test |
| `half_failure_does_not_block_successful_deposits` | No | Validates partial-failure isolation |
| `slow_listener_100_addresses_completes_within_budget` | No | Wall-clock budget for 100 slow addresses |
| `db_100_wallets_listener_returns_100_deposits` | **Yes** | End-to-end: seed 100 wallets, run poll cycle |
| `db_half_failure_still_processes_good_addresses` | **Yes** | End-to-end partial failure |
| `documents_scale_limits` | No | Prints theoretical throughput ceiling |

### Running

```bash
# In-memory tests only (no database needed):
cargo test --test deposit_polling_worker_load

# All tests including database-backed ones:
docker exec -i aframp-postgres psql -U postgres -c "CREATE DATABASE aframp_test;"
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/aframp_test \
    cargo test --test deposit_polling_worker_load -- --ignored
```

---

## Mock Listeners

Three mock `BlockchainListener` implementations are provided in the test file:

### `AllSucceedListener`

Returns one synthetic `DetectedDeposit` per address. Used to verify that the
worker credits every wallet in a full poll cycle.

### `HalfErrorListener`

Returns deposits only for **odd-indexed** addresses (50% success rate). Used to
verify that per-address failures are isolated — a bad address does not prevent
the other 50% from being processed.

### `SlowListener`

Sleeps for a configurable delay before returning. Used to verify the worker does
not deadlock or time out when Horizon is slow.

---

## Scale Limits

The following analysis assumes the default configuration:

| Parameter | Value | Source |
|-----------|-------|--------|
| `STELLAR_POLL_INTERVAL_SECS` | 60 | `.env.example` |
| Estimated Horizon latency | ~200 ms | Observed on testnet |
| Worker concurrency | 1 (sequential) | `src/blockchain/stellar.rs` |

### Sequential ceiling

With sequential per-address fetches:

```
max_wallets = poll_interval_ms / per_address_latency_ms
            = 60,000 ms / 200 ms
            = 300 wallets per cycle
```

At 300+ active wallets, the worker starts falling behind — the previous cycle
is still running when the next one is due to start.

### Concurrent ceiling

If fetches were parallelised (e.g. `tokio::spawn` per address, or a bounded
`FuturesUnordered`):

```
max_wallets (×10 concurrent) = 300 × 10 = 3,000 wallets per cycle
```

### Recommended changes (tracked in this issue)

1. **Batch concurrent fetches.** Use `futures::stream::FuturesUnordered` or
   `tokio::task::JoinSet` with a bounded concurrency limit (e.g. 20 simultaneous
   Horizon requests).
2. **Add tracing spans** for per-cycle wallet count and total duration — make
   falling-behind visible before it becomes a problem.
3. **Per-cycle metrics.** Record `deposits_detected`, `addresses_polled`,
   `addresses_skipped` (backoff), and `addresses_errored` per cycle.
4. **Increase `STELLAR_POLL_INTERVAL_SECS`** for large deployments if concurrency
   cannot be added (buys time, not a fix).

---

## Known Gaps

- `poll_once` in `src/blockchain/worker.rs` is `async fn` but not `pub`, so the
  test harness cannot call it directly. The DB-backed tests work around this by
  testing the `BlockchainListener` boundary only. A future refactor could export
  a testable `poll_cycle(db, listener)` helper.
- The `StellarListener` uses an internal `Mutex<HashMap>` for backoff state,
  which is safe for the current single-threaded polling design but becomes a
  contention point if fetches are parallelised. The backoff map should be
  replaced with `DashMap` or a per-address `RwLock` before introducing
  concurrency.
- There is no circuit breaker: if Horizon is down for a full poll interval,
  the worker silently retries next cycle. A counter of consecutive global
  failures + alerting would improve operational visibility.
