-- Web push subscriptions per merchant (issue #1346)
-- Stores the browser PushSubscription (endpoint + keys) so the API can
-- deliver notifications for confirmed payments and withdrawal status changes.

CREATE TABLE IF NOT EXISTS push_subscriptions (
    id          BIGSERIAL PRIMARY KEY,
    merchant_id BIGINT NOT NULL REFERENCES merchants (id) ON DELETE CASCADE,
    endpoint    TEXT NOT NULL,
    p256dh      TEXT NOT NULL,
    auth        TEXT NOT NULL,
    user_agent  TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT push_subscriptions_merchant_endpoint_key UNIQUE (merchant_id, endpoint)
);

CREATE INDEX IF NOT EXISTS push_subscriptions_merchant_id_idx
    ON push_subscriptions (merchant_id);
