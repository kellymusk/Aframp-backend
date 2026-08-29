ALTER TABLE wallets
  ADD COLUMN secret_key_encrypted TEXT NOT NULL DEFAULT '';

ALTER TABLE wallets
  ALTER COLUMN secret_key_encrypted DROP DEFAULT;
