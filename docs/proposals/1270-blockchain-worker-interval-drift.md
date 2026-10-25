# Proposal: Fix Blockchain Worker Interval Drift (#1270)

## Problem

In `src/blockchain/worker.rs` the poll loop is structured as:

```rust
loop {
    if let Err(err) = poll_once(...).await { ... }
    tokio::time::sleep(Duration::from_secs(poll_interval_secs)).await;
}
```

This causes two related problems:

1. **Interval drift** — if `poll_once` takes 45 s and `poll_interval_secs` is 60,
   the effective poll frequency is 105 s, not 60 s. The sleep runs *after* the poll,
   so poll duration adds directly to the configured interval.

2. **Undetected overrun** — if `poll_once` takes longer than the interval (e.g. 65 s
   on a slow Horizon endpoint with many wallets) there is no warning. The sleep still
   fires and the operator has no signal that polling is lagging.

## Proposed Fix

Replace the `sleep`-after-poll pattern with `tokio::time::interval` configured with
`MissedTickBehavior::Delay`. `Delay` fires the *next* tick at `interval` after the
last tick completes, so a slow poll simply delays the next one — it never causes
overlapping polls. The poll duration is recorded and logged so operators can detect
overrun.

```rust
use std::time::Instant;
use tokio::time::{interval, Duration, MissedTickBehavior};

pub async fn run(state: Arc<AppState>, horizon_url: String, poll_interval_secs: u64) {
    let listener = StellarListener::new(horizon_url);
    let period = Duration::from_secs(poll_interval_secs);
    let mut ticker = interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;

        let start = Instant::now();
        if let Err(err) = poll_once(&state.db, &listener).await {
            tracing::warn!(error = %err, "deposit poll failed");
        }
        let elapsed = start.elapsed();
        tracing::debug!(elapsed_ms = elapsed.as_millis(), "deposit poll completed");

        if elapsed > period {
            tracing::warn!(
                elapsed_ms = elapsed.as_millis(),
                interval_ms = period.as_millis(),
                "deposit poll took longer than the configured interval — polling is lagging"
            );
        }
    }
}
```

## Behaviour Changes

| Scenario | Before | After |
|---|---|---|
| Poll takes 45 s, interval 60 s | Effective frequency: 105 s | Effective frequency: 60 s (next tick fires 60 s after the last tick *started*) |
| Poll takes 65 s, interval 60 s | Silent overrun; sleep still runs | Warning logged; next tick fires immediately after poll finishes |
| Poll takes 1 s, interval 60 s | Correct (59 s sleep) | Correct (next tick 59 s after poll start) |

## Why `MissedTickBehavior::Delay` and not `Burst`

`Burst` would fire missed ticks immediately back-to-back on recovery, potentially
hammering Horizon with catch-up polls. `Delay` simply reschedules to `now + interval`,
which is the safest behaviour for an I/O polling loop.

## References

- Tokio docs: [`MissedTickBehavior`](https://docs.rs/tokio/latest/tokio/time/enum.MissedTickBehavior.html)
- Issue: #1270
