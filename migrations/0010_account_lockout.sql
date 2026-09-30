-- #1079: Add account lockout columns to users table.
-- failed_login_count: incremented on each failed password attempt, reset on success.
-- locked_until: set to now() + 30 minutes after 10 consecutive failures; NULL = not locked.
ALTER TABLE users ADD COLUMN IF NOT EXISTS failed_login_count INT NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN IF NOT EXISTS locked_until TIMESTAMPTZ;
