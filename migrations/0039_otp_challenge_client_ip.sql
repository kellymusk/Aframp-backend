-- #1308: the address that requested an OTP challenge, for audit. NULL for
-- older challenges and when the address isn't known.
ALTER TABLE otp_challenges ADD COLUMN IF NOT EXISTS client_ip INET;
CREATE INDEX IF NOT EXISTS otp_challenges_client_ip_created_at_idx
  ON otp_challenges (client_ip, created_at DESC)
  WHERE client_ip IS NOT NULL;
