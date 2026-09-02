-- Add 'partial' and 'underpaid' status to payment_requests
ALTER TABLE payment_requests
DROP CONSTRAINT payment_requests_status_check,
ADD CONSTRAINT payment_requests_status_check CHECK (status IN ('pending', 'paid', 'partial', 'expired'));
