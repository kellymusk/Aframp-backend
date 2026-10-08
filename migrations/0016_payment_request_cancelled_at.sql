-- Soft-delete / archive for payment requests (#1133).
-- Merchants cancel via DELETE; rows stay until a cleanup job hard-deletes
-- expired+cancelled requests older than 30 days.
ALTER TABLE payment_requests
  ADD COLUMN cancelled_at TIMESTAMPTZ;

CREATE INDEX idx_payment_requests_cancelled_at
  ON payment_requests (cancelled_at)
  WHERE cancelled_at IS NOT NULL;
