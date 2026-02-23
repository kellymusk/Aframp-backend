-- migrate:up
-- Extend bill_payments table with bill processor state and token management

-- Add columns for bill processor state tracking
ALTER TABLE bill_payments
ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'pending_payment' CHECK (status IN (
    'pending_payment',
    'cngn_received',
    'verifying_account',
    'account_invalid',
    'processing_bill',
    'provider_processing',
    'completed',
    'retry_scheduled',
    'provider_failed',
    'refund_initiated',
    'refund_processing',
    'refunded'
)),
ADD COLUMN IF NOT EXISTS provider_reference TEXT,
ADD COLUMN IF NOT EXISTS token TEXT,
ADD COLUMN IF NOT EXISTS provider_response JSONB,
ADD COLUMN IF NOT EXISTS retry_count INTEGER NOT NULL DEFAULT 0,
ADD COLUMN IF NOT EXISTS last_retry_at TIMESTAMPTZ,
ADD COLUMN IF NOT EXISTS error_message TEXT,
ADD COLUMN IF NOT EXISTS refund_tx_hash TEXT,
ADD COLUMN IF NOT EXISTS account_verified BOOLEAN DEFAULT FALSE,
ADD COLUMN IF NOT EXISTS verification_data JSONB;

-- Create indexes for efficient querying
CREATE INDEX IF NOT EXISTS idx_bill_payments_status ON bill_payments(status);
CREATE INDEX IF NOT EXISTS idx_bill_payments_created_at ON bill_payments(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_bill_payments_retry_eligible ON bill_payments(status, last_retry_at) 
    WHERE status = 'retry_scheduled';
CREATE INDEX IF NOT EXISTS idx_bill_payments_pending ON bill_payments(status)
    WHERE status IN ('cngn_received', 'verifying_account', 'processing_bill', 'provider_processing');

-- Add comments for documentation
COMMENT ON COLUMN bill_payments.status IS 'Current processing state of the bill payment transaction';
COMMENT ON COLUMN bill_payments.provider_reference IS 'Reference ID from the provider (tx_ref, transaction ID, etc.)';
COMMENT ON COLUMN bill_payments.token IS 'Delivery token (electricity tokens, etc.)';
COMMENT ON COLUMN bill_payments.provider_response IS 'Full JSON response from provider API';
COMMENT ON COLUMN bill_payments.retry_count IS 'Number of retry attempts made';
COMMENT ON COLUMN bill_payments.last_retry_at IS 'Timestamp of last retry attempt';
COMMENT ON COLUMN bill_payments.error_message IS 'Error details if payment failed';
COMMENT ON COLUMN bill_payments.refund_tx_hash IS 'Stellar transaction hash for refund';
COMMENT ON COLUMN bill_payments.account_verified IS 'Whether account was verified successfully';
COMMENT ON COLUMN bill_payments.verification_data IS 'Account verification response data';

-- Create view for monitoring bill payments by status
CREATE OR REPLACE VIEW bill_payments_by_status AS
SELECT 
    status,
    COUNT(*) as count,
    COUNT(CASE WHEN error_message IS NOT NULL THEN 1 END) as with_errors,
    AVG(retry_count) as avg_retries,
    MAX(updated_at) as last_updated
FROM bill_payments
WHERE updated_at > NOW() - INTERVAL '24 hours'
GROUP BY status;

-- Create view for monitoring payment success rates
CREATE OR REPLACE VIEW bill_payments_success_rate AS
SELECT 
    bill_type,
    COUNT(*) as total,
    COUNT(CASE WHEN status = 'completed' THEN 1 END) as completed,
    COUNT(CASE WHEN status = 'refunded' THEN 1 END) as refunded,
    COUNT(CASE WHEN status = 'refunded' THEN 1 END)::float / NULLIF(COUNT(*), 0) as refund_rate,
    COUNT(CASE WHEN status = 'completed' THEN 1 END)::float / NULLIF(COUNT(*), 0) as success_rate
FROM bill_payments
WHERE created_at > NOW() - INTERVAL '24 hours'
GROUP BY bill_type;

-- Create view for monitoring provider performance
CREATE OR REPLACE VIEW provider_performance AS
SELECT 
    provider_name,
    COUNT(*) as total_payments,
    COUNT(CASE WHEN status = 'completed' THEN 1 END) as successful,
    COUNT(CASE WHEN status IN ('refunded', 'provider_failed') THEN 1 END) as failed,
    COUNT(CASE WHEN status = 'completed' THEN 1 END)::float / NULLIF(COUNT(*), 0) as success_rate,
    AVG(EXTRACT(EPOCH FROM (updated_at - created_at))) as avg_duration_seconds,
    MAX(retry_count) as max_retries
FROM bill_payments
WHERE created_at > NOW() - INTERVAL '24 hours'
GROUP BY provider_name;

-- Function to mark bill payment as ready for retry
CREATE OR REPLACE FUNCTION mark_bill_payment_ready_for_retry(
    bill_id UUID,
    backoff_seconds INTEGER
) RETURNS BOOLEAN AS $$
BEGIN
    UPDATE bill_payments
    SET 
        status = 'processing_bill',
        retry_count = retry_count + 1,
        last_retry_at = NOW(),
        updated_at = NOW()
    WHERE 
        id = bill_id
        AND status = 'retry_scheduled'
        AND (last_retry_at IS NULL OR last_retry_at + INTERVAL '1 second' * backoff_seconds <= NOW());
    
    RETURN FOUND;
END;
$$ LANGUAGE plpgsql;

-- Function to transition bill payment to refund
CREATE OR REPLACE FUNCTION transition_bill_to_refund(
    bill_id UUID,
    error_reason TEXT
) RETURNS BOOLEAN AS $$
BEGIN
    UPDATE bill_payments
    SET 
        status = 'refund_initiated',
        error_message = error_reason,
        updated_at = NOW()
    WHERE id = bill_id;
    
    RETURN FOUND;
END;
$$ LANGUAGE plpgsql;

-- Function to mark refund as processed
CREATE OR REPLACE FUNCTION mark_refund_processed(
    bill_id UUID,
    refund_hash TEXT
) RETURNS BOOLEAN AS $$
BEGIN
    UPDATE bill_payments
    SET 
        status = 'refunded',
        refund_tx_hash = refund_hash,
        updated_at = NOW()
    WHERE id = bill_id;
    
    RETURN FOUND;
END;
$$ LANGUAGE plpgsql;

