-- Add UNIQUE constraint on (wallet_id, tx_hash) to prevent double-processing of deposits
-- Drop the existing UNIQUE constraint on tx_hash only
ALTER TABLE payments
DROP CONSTRAINT payments_tx_hash_key,
ADD UNIQUE (wallet_id, tx_hash);
