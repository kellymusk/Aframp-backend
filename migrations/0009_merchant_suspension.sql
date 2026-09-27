-- Migration: 0009_merchant_suspension
-- Adds suspension support for merchant accounts (issue #1080).
-- A non-NULL suspended_at marks the merchant as suspended; NULL means active.

ALTER TABLE merchants
    ADD COLUMN suspended_at TIMESTAMPTZ;

-- Index to make suspension checks in /login, /payment-requests and /withdraw fast.
CREATE INDEX idx_merchants_suspended_at ON merchants (suspended_at);
