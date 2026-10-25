-- #1308: retain the address that requested an OTP challenge for audit and
-- future IP-based rate limiting. Existing challenges have no known address.
ALTER TABLE otp_challenges ADD COLUMN client_ip INET;

CREATE INDEX otp_challenges_client_ip_created_at_idx
  ON otp_challenges (client_ip, created_at DESC)
  WHERE client_ip IS NOT NULL;
