-- `webhook_events` is the idempotency ledger for incoming provider callbacks:
-- the handler inserts one row per event before acting on it, and the
-- `UNIQUE (provider, external_id)` constraint is what makes a Paystack retry
-- of an already-processed event a no-op instead of a double payout.
--
-- Two things about the original shape blocked that from actually working:
--
--   1. `merchant_id` was NOT NULL, but Paystack events are platform-level.
--      They arrive on one integration-wide webhook URL and carry no merchant
--      of ours; the merchant is only discoverable *after* parsing the payload
--      and matching `data.reference` back to a withdrawal — which is work the
--      handler must not have to do before it can dedupe. Worse, an event we
--      cannot attribute (an unknown reference, a `charge.*` event for
--      something we did not initiate) had no legal row at all, so the one
--      case where deduplication matters most could not be recorded.
--
--   2. `external_id` is Paystack's own event `id` field. Nothing documented
--      that, so a future handler could just as easily have stored
--      `data.reference` there — two different events sharing a reference
--      would then collide and the second would be silently dropped.
--
-- `merchant_id` becomes nullable: NULL means "platform-level event, not (yet)
-- attributed to a merchant". Rows we can attribute still carry the id, so the
-- FK and per-merchant queries keep working.

ALTER TABLE webhook_events ALTER COLUMN merchant_id DROP NOT NULL;

COMMENT ON TABLE webhook_events IS
  'Idempotency ledger for inbound provider webhooks. Insert before processing; a unique violation on (provider, external_id) means this event was already handled and must be acknowledged without re-processing.';

COMMENT ON COLUMN webhook_events.merchant_id IS
  'The merchant this event resolved to, or NULL for a platform-level event not attributed to one. Nullable because Paystack events arrive integration-wide and must be recorded for deduplication before attribution is attempted.';

COMMENT ON COLUMN webhook_events.provider IS
  'Payment provider that sent the event, e.g. ''paystack''. Namespaces external_id so two providers reusing an id do not collide.';

COMMENT ON COLUMN webhook_events.external_id IS
  'The provider''s own immutable event identifier — for Paystack, the top-level ''id'' field of the event body, NOT ''data.reference''. A reference is per-transfer and repeats across the event types of one transfer''s lifecycle; the event id is unique per delivery and is what makes retries idempotent.';

COMMENT ON COLUMN webhook_events.payload IS
  'The verbatim event body as received, retained for replay and dispute investigation. Store the parsed JSON of the exact bytes the signature was computed over.';

-- Attribution happens after insert, so the merchant lookup path needs an index
-- of its own; the unique constraint above only covers (provider, external_id).
CREATE INDEX IF NOT EXISTS webhook_events_merchant_id_idx
  ON webhook_events (merchant_id)
  WHERE merchant_id IS NOT NULL;
