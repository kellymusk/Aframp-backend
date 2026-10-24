-- Self-service merchant email change (PATCH /merchants/me). A requested
-- address is held as pending until the owner confirms the token sent to it,
-- so an account can't be pointed at an inbox its owner doesn't control.
ALTER TABLE merchants
  ADD COLUMN pending_email TEXT,
  ADD COLUMN email_change_token_hash TEXT,
  ADD COLUMN email_change_expires_at TIMESTAMPTZ;
