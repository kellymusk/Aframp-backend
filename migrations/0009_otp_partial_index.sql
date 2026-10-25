-- Migration: 0009_otp_partial_index
--
-- Context (#1259):
-- `upsert_challenge` in src/services/otp.rs runs a query that filters on:
--   WHERE phone_number = $1 AND purpose = $2 AND consumed_at IS NULL AND expires_at > now()
--
-- The existing index `otp_challenges_phone_purpose_idx` covers (phone_number, purpose, created_at DESC),
-- which handles the hourly-count query well, but the live-challenge lookup adds
-- `consumed_at IS NULL` and `expires_at > now()` predicates that the index cannot
-- filter — Postgres applies them as a heap filter after the index scan, scanning
-- every unconsumed+unexpired row for that phone+purpose pair.
--
-- A partial index scoped to `WHERE consumed_at IS NULL` dramatically reduces the
-- index size (only live, unverified challenges are indexed) and lets Postgres
-- satisfy the entire predicate from the index, avoiding the heap filter entirely.
-- Expired challenges remain in the index until consumed or cleaned up, but the
-- `expires_at > now()` predicate is still evaluated as a filter — this is
-- intentional: a partial index on a moving target like `expires_at` would
-- require constant maintenance. The dominant cost is scanning many unconsumed
-- rows, which the partial index eliminates.
CREATE INDEX otp_challenges_live_phone_purpose_idx
    ON otp_challenges (phone_number, purpose, created_at DESC)
    WHERE consumed_at IS NULL;
