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
