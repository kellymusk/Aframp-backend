-- #1315: Termii delivery callbacks are recorded in webhook_events too, as an
-- audit trail. event_type is the provider's event or status name.
ALTER TABLE webhook_events ADD COLUMN IF NOT EXISTS event_type TEXT;
CREATE INDEX IF NOT EXISTS webhook_events_provider_received_idx
  ON webhook_events (provider, received_at DESC);
