-- #1287: deposits that look anomalous (one very large deposit, or a burst
-- within an hour) are still credited but flagged for admin review.
ALTER TABLE payments ADD COLUMN IF NOT EXISTS flagged BOOLEAN NOT NULL DEFAULT false;
CREATE INDEX IF NOT EXISTS payments_flagged_idx ON payments (created_at DESC) WHERE flagged;
