-- Audit trail of every decryption of a wallet's secret_key_encrypted.
-- A leaked WALLET_ENCRYPTION_KEY exposes every merchant key, so each use of
-- the key is recorded (which wallet, when, and why) to make unexpected
-- access detectable. Rows are written by services::wallets::decrypt_secret
-- and by key rotation.

CREATE TABLE wallet_secret_access_log (
  id BIGSERIAL PRIMARY KEY,
  wallet_id UUID NOT NULL REFERENCES wallets(id) ON DELETE CASCADE,
  purpose TEXT NOT NULL,
  accessed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_wallet_secret_access_log_wallet
    ON wallet_secret_access_log (wallet_id, accessed_at DESC);
