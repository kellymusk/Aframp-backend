-- Self-service email change (PATCH /merchants/me). The requested address is
-- held as pending until the owner presents the token sent to it, so an
-- account can't be pointed at an inbox its owner doesn't control.
ALTER TABLE users
  ADD COLUMN IF NOT EXISTS pending_email TEXT,
  ADD COLUMN IF NOT EXISTS email_change_token_hash TEXT,
  ADD COLUMN IF NOT EXISTS email_change_expires_at TIMESTAMPTZ;
