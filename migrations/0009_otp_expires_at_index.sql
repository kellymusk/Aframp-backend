-- #1077: Add index on otp_challenges(expires_at) for efficient cleanup queries.
--
-- Retention policy: OTP audit records are kept for 24 hours after expiry.
-- A background cleanup task deletes rows where expires_at < now() - interval '24 hours'.
-- This gives a 24-hour window for auditing / debugging failed challenges while
-- preventing unbounded table growth at high user volume.
CREATE INDEX IF NOT EXISTS otp_challenges_expires_at_idx ON otp_challenges (expires_at);
