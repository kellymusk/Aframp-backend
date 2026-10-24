-- Migration 0009: make webhook_events a real audit trail
--
-- Decision (issue #1315): we KEEP the webhook_events table and populate it
-- from the webhook handlers instead of dropping it. The table previously
-- existed but was never written to, which created a misleading audit gap.
--
-- Termii webhooks are not merchant-specific, so merchant_id must be nullable.
-- We also add provider/event_type/raw_payload columns so the audit record is
-- self-describing even when no merchant can be resolved.

-- Allow non-merchant-specific events (e.g. Termii delivery callbacks).
ALTER TABLE webhook_events
  ALTER COLUMN merchant_id DROP NOT NULL;

-- Provider that emitted the event (e.g. 'termii', 'paystack').
ALTER TABLE webhook_events
  ADD COLUMN IF NOT EXISTS provider TEXT;

-- Normalized event type (e.g. 'delivery_report', 'charge.success').
ALTER TABLE webhook_events
  ADD COLUMN IF NOT EXISTS event_type TEXT;

-- Raw payload as received, for forensic/audit purposes.
ALTER TABLE webhook_events
  ADD COLUMN IF NOT EXISTS raw_payload JSONB;

-- Helpful index for auditing by provider over time.
CREATE INDEX IF NOT EXISTS idx_webhook_events_provider_created_at
  ON webhook_events (provider, created_at DESC);
