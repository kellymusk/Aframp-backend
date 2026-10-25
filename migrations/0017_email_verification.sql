-- Email verification for merchant signup (#1232).
--
-- Accounts are created once the phone OTP challenge passes (see
-- services::otp::verify); the email is verified out-of-band via a tokenised
-- link so a typo'd address can be caught before transactional email is lost.

ALTER TABLE users
  ADD COLUMN IF NOT EXISTS email_verified BOOLEAN NOT NULL DEFAULT false,
  ADD COLUMN IF NOT EXISTS email_verification_token TEXT,
  ADD COLUMN IF NOT EXISTS email_verification_expires_at TIMESTAMPTZ;

-- Only one live token per address; lookups are by hash.
CREATE INDEX IF NOT EXISTS users_email_verification_token_idx
  ON users (email_verification_token)
  WHERE email_verification_token IS NOT NULL;
