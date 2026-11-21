# Proposal: persistent retry queue for merchant webhooks (#1447)

## Problem
`deliver()` retries in-process with `tokio::time::sleep` (5 attempts, 1s base, doubling: 1+2+4+8 = 15s worst case). Each failing delivery holds a spawned task for ~15s, retries are lost on restart, and many failing endpoints pile up tasks.

## Proposal
- Add a `webhook_deliveries` table: `id, merchant_id, event, payload, attempt, next_attempt_at, status, last_error`.
- `deliver()` performs one attempt; on failure it records `next_attempt_at = now() + backoff(attempt)` and returns.
- A worker polls `SELECT ... WHERE status = 'pending' AND next_attempt_at <= now() FOR UPDATE SKIP LOCKED LIMIT n` and retries.
- After `max_attempts`, mark `failed` for operator inspection/replay.

## Benefits
No long-lived tasks, retries survive restarts, backoff can extend to minutes/hours, and delivery history is auditable.
