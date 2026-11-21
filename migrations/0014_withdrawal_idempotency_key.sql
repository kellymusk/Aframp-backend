-- Add idempotency key support to withdrawals to prevent duplicate payouts on retry.
-- A client may send an optional `Idempotency-Key` header on POST /withdraw.
-- When a matching (merchant_id, idempotency_key) pair already exists, the
-- existing withdrawal is returned instead of creating a new one.

ALTER TABLE withdrawals
    ADD COLUMN idempotency_key TEXT;

-- Enforce uniqueness per merchant so a retried request cannot create a second
-- withdrawal. NULL keys are allowed (idempotency is opt-in) and, per SQL
-- semantics, multiple NULLs do not conflict with each other.
CREATE UNIQUE INDEX idx_withdrawals_merchant_idempotency_key
    ON withdrawals (merchant_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

-- Serialise the daily-limit check in `create_withdrawal_idempotent()`.
--
-- The daily-limit query counts withdrawals with `status <> 'failed'` created
-- since UTC midnight. Under concurrent requests, sibling transactions that have
-- not yet committed are invisible to each other, so a burst of withdrawals can
-- each pass the limit check before any of them commits and the daily limit is
-- bypassed. The balance debit's row lock only serialises sequential debits, not
-- the pre-debit limit read.
--
-- A per-merchant advisory lock taken before the daily-total query forces the
-- check + insert to run one-at-a-time per merchant, so each request observes
-- the withdrawals committed by its predecessors. The lock is transaction-scoped
-- and released automatically on COMMIT/ROLLBACK.
--
-- Supporting index so the daily-total query stays cheap under the lock.
CREATE INDEX idx_withdrawals_merchant_created_at
    ON withdrawals (merchant_id, created_at)
    WHERE status <> 'failed';
