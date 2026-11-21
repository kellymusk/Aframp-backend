-- Add UNIQUE constraint on (wallet_id, tx_hash) to prevent double-processing of deposits
-- Drop the existing UNIQUE constraint on tx_hash only
ALTER TABLE payments
DROP CONSTRAINT payments_tx_hash_key,
ADD UNIQUE (wallet_id, tx_hash);

-- Also enforce uniqueness on (tx_hash, wallet_address) so a re-org replay of the
-- same transaction hash to a different destination address is not silently ignored.
CREATE UNIQUE INDEX IF NOT EXISTS payments_tx_hash_wallet_address_key
ON payments (tx_hash, wallet_address);
