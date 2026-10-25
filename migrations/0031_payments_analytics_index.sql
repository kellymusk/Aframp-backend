-- GET /analytics/payments filters a merchant's confirmed payments by
-- created_at range; this index keeps that a single range scan.
CREATE INDEX IF NOT EXISTS payments_merchant_status_created_idx
    ON payments (merchant_id, status, created_at);
