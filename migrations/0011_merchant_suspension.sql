-- #1080: Add suspended_at column to merchants table.
-- NULL means active; a non-NULL timestamp records when the merchant was suspended.
ALTER TABLE merchants ADD COLUMN IF NOT EXISTS suspended_at TIMESTAMPTZ;
