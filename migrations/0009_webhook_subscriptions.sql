-- Webhook subscriptions: per-merchant self-service webhook URL management
CREATE TABLE IF NOT EXISTS webhook_subscriptions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_id UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    events TEXT[] NOT NULL DEFAULT '{}',
    secret_hash TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_webhook_subscriptions_merchant_id
    ON webhook_subscriptions (merchant_id);

CREATE INDEX IF NOT EXISTS idx_webhook_subscriptions_active
    ON webhook_subscriptions (merchant_id, active);
