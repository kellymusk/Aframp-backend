-- Issue #1415: dedup deposits on (tx_hash, wallet_address) instead of tx_hash alone.
-- A re-org replay of the same tx_hash to a different destination wallet must be
-- recorded as a distinct payment rather than silently ignored.

-- Drop the old single-column uniqueness (if it exists) so the compound key governs dedup.
DROP INDEX IF EXISTS unique_payment_wallet_tx_hash;

-- Remove any pre-existing single-column unique constraint/index on tx_hash alone.
ALTER TABLE payments DROP CONSTRAINT IF EXISTS payments_tx_hash_key;
DROP INDEX IF EXISTS payments_tx_hash_key;
DROP INDEX IF EXISTS idx_payments_tx_hash_unique;

-- Compound uniqueness: one payment per (tx_hash, destination wallet address).
CREATE UNIQUE INDEX IF NOT EXISTS unique_payment_wallet_tx_hash
    ON payments (tx_hash, wallet_address);
