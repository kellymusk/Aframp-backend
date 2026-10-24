-- Migration 0009: add optional merchant-defined description to payment requests
-- Issue #1292: support merchant-defined payment request descriptions

ALTER TABLE payment_requests
    ADD COLUMN description TEXT;
