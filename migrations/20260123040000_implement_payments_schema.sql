-- migrate:up
-- #5 Implement Payment Provider and Bill Payment Schema
-- Purpose: Create schema for tracking payment provider integrations, bill payment operations, and webhook events.
-- Requirements: 
-- - payment_methods table for user preferences
-- - bill_payments table for bill tracking
-- - webhook_events table for immutable log
-- - payment_provider_configs table for settings

-- 1. Payment Provider Configs
CREATE TABLE payment_provider_configs (
    provider TEXT PRIMARY KEY,
    is_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    settings JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE payment_provider_configs IS 'Configuration settings for different payment providers.';
COMMENT ON COLUMN payment_provider_configs.provider IS 'Unique identifier for the payment provider (e.g., flutterwave, paystack, mpesa).';
COMMENT ON COLUMN payment_provider_configs.is_enabled IS 'Flag to enable or disable the provider globally.';
COMMENT ON COLUMN payment_provider_configs.settings IS 'Provider-specific settings and configurations stored as JSONB.';

-- Seed initial providers
INSERT INTO payment_provider_configs (provider) VALUES 
('flutterwave'), ('paystack'), ('mpesa')
ON CONFLICT (provider) DO NOTHING;

-- 2. Payment Methods
CREATE TABLE payment_methods (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider TEXT NOT NULL REFERENCES payment_provider_configs(provider),
    method_type TEXT NOT NULL CHECK (method_type IN ('mpesa', 'bank', 'card')),
    phone_number TEXT,
    encrypted_data TEXT, 
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
    region TEXT, 
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE payment_methods IS 'User payment preferences and stored methods (MPESA, Bank, or Card).';
COMMENT ON COLUMN payment_methods.user_id IS 'Links users to their preferred payment methods.';
COMMENT ON COLUMN payment_methods.provider IS 'The payment provider used for this method.';
COMMENT ON COLUMN payment_methods.method_type IS 'The type of payment (mpesa, bank, or card).';
COMMENT ON COLUMN payment_methods.phone_number IS 'Associated phone number for the payment method (e.g., for MPESA).';
COMMENT ON COLUMN payment_methods.encrypted_data IS 'PCI-compliant storage for sensitive data (tokens), never store raw card details.';
COMMENT ON COLUMN payment_methods.is_active IS 'Flag indicating if the payment method is currently active.';
COMMENT ON COLUMN payment_methods.is_deleted IS 'Soft delete flag to preserve history while removing from user view.';
COMMENT ON COLUMN payment_methods.region IS 'The region or country where this method is available.';

-- 3. Bill Payments
CREATE TABLE bill_payments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    transaction_id UUID NOT NULL UNIQUE REFERENCES transactions(transaction_id) ON DELETE CASCADE,
    provider_name TEXT NOT NULL,
    account_number TEXT NOT NULL,
    bill_type TEXT NOT NULL CHECK (bill_type IN ('electricity', 'water', 'airtime', 'internet', 'cable_tv')),
    due_date TIMESTAMPTZ,
    paid_with_afri BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE bill_payments IS 'Details for transaction operations of type bill_payment.';
COMMENT ON COLUMN bill_payments.transaction_id IS 'Foreign key linking this bill payment to the core transactions table.';
COMMENT ON COLUMN bill_payments.provider_name IS 'The specific provider used for the bill payment.';
COMMENT ON COLUMN bill_payments.account_number IS 'The customer account number for the bill (e.g., meter number).';
COMMENT ON COLUMN bill_payments.bill_type IS 'The type of bill being paid (electricity, water, airtime, etc.).';
COMMENT ON COLUMN bill_payments.due_date IS 'Optional due date for the bill.';
COMMENT ON COLUMN bill_payments.paid_with_afri IS 'True if the payment was made using the AFRI stablecoin.';

-- 4. Webhook Events
CREATE TABLE webhook_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id TEXT NOT NULL, 
    provider TEXT NOT NULL REFERENCES payment_provider_configs(provider),
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    signature TEXT,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'processing', 'completed', 'failed')),
    transaction_id UUID REFERENCES transactions(transaction_id), 
    processed_at TIMESTAMPTZ,
    retry_count INTEGER NOT NULL DEFAULT 0,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (provider, event_id)
);

COMMENT ON TABLE webhook_events IS 'Immutable log of all incoming webhooks from payment providers for audit and idempotency.';
COMMENT ON COLUMN webhook_events.event_id IS 'Unique identifier for the event assigned by the provider to prevent duplicate processing.';
COMMENT ON COLUMN webhook_events.provider IS 'The provider source of the webhook.';
COMMENT ON COLUMN webhook_events.event_type IS 'The specific type of event (e.g., payment.success).';
COMMENT ON COLUMN webhook_events.payload IS 'The full JSONB payload received from the provider.';
COMMENT ON COLUMN webhook_events.signature IS 'The cryptographic signature for verifying the webhook authenticity.';
COMMENT ON COLUMN webhook_events.status IS 'Current processing state of the webhook event.';
COMMENT ON COLUMN webhook_events.transaction_id IS 'Optional reference to a specific transaction linked to this webhook.';
COMMENT ON COLUMN webhook_events.processed_at IS 'When the webhook processing was finished.';
COMMENT ON COLUMN webhook_events.retry_count IS 'Number of retries in case of processing failure.';
COMMENT ON COLUMN webhook_events.error_message IS 'Error description if the status is failed.';

-- 5. Webhook Deliveries (Optional/Consider Note)
CREATE TABLE webhook_deliveries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id UUID NOT NULL REFERENCES webhook_events(id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'delivered', 'failed')),
    response_code INTEGER,
    response_body TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE webhook_deliveries IS 'Tracking for outgoing webhooks sent to user-configured endpoints or partner systems.';
COMMENT ON COLUMN webhook_deliveries.event_id IS 'The source webhook event being delivered.';
COMMENT ON COLUMN webhook_deliveries.url IS 'The target destination URL.';
COMMENT ON COLUMN webhook_deliveries.status IS 'Delivery status.';
COMMENT ON COLUMN webhook_deliveries.response_code IS 'HTTP response code from the receiver.';
COMMENT ON COLUMN webhook_deliveries.response_body IS 'Response body for debugging failures.';

-- Triggers to maintain updated_at
CREATE TRIGGER set_updated_at_payment_provider_configs
    BEFORE UPDATE ON payment_provider_configs
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER set_updated_at_payment_methods
    BEFORE UPDATE ON payment_methods
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER set_updated_at_bill_payments
    BEFORE UPDATE ON bill_payments
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER set_updated_at_webhook_events
    BEFORE UPDATE ON webhook_events
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER set_updated_at_webhook_deliveries
    BEFORE UPDATE ON webhook_deliveries
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Indexes for performance
CREATE INDEX idx_payment_methods_user_id ON payment_methods(user_id);
CREATE INDEX idx_payment_methods_active_non_deleted ON payment_methods(user_id, is_active) WHERE is_deleted IS FALSE;
CREATE INDEX idx_bill_payments_transaction_id ON bill_payments(transaction_id);
CREATE INDEX idx_webhook_events_event_type ON webhook_events(event_type);
CREATE INDEX idx_webhook_events_created_at ON webhook_events(created_at);
CREATE INDEX idx_webhook_events_status_pending ON webhook_events(status, created_at) WHERE status = 'pending';
CREATE INDEX idx_webhook_events_transaction_query ON webhook_events(transaction_id) WHERE transaction_id IS NOT NULL;
CREATE INDEX idx_webhook_deliveries_event_id ON webhook_deliveries(event_id);
CREATE INDEX idx_webhook_deliveries_status ON webhook_deliveries(status) WHERE status != 'delivered';

-- migrate:down
DROP TABLE IF EXISTS webhook_deliveries;
DROP TABLE IF EXISTS webhook_events;
DROP TABLE IF EXISTS bill_payments;
DROP TABLE IF EXISTS payment_methods;
DROP TABLE IF EXISTS payment_provider_configs;
