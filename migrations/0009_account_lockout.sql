-- Account lockout after N failed password attempts (issue #1079)
-- Adds tracking columns to the users table so the login flow can
-- increment a failure counter and lock the account for a period.

ALTER TABLE users
    ADD COLUMN failed_login_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN locked_until TIMESTAMPTZ;

-- Speed up lookups of currently locked accounts (admin unlock / audits).
CREATE INDEX idx_users_locked_until ON users (locked_until)
    WHERE locked_until IS NOT NULL;
