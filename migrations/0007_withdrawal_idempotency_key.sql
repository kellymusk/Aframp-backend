-- Backs the Idempotency-Key header on POST /withdraw: a merchant retrying a
-- slow/ambiguous request supplies the same key, and the second request must
-- return the original withdrawal rather than create a second one. NULL for
-- requests sent without the header (idempotency is opt-in, matching the
-- header's optional status in the API).
ALTER TABLE withdrawals ADD COLUMN idempotency_key TEXT;

-- Scoped per merchant: the header is a client-generated ID with no global
-- uniqueness guarantee across merchants, and merchant_id is part of every
-- other withdrawal lookup already.
CREATE UNIQUE INDEX withdrawals_merchant_idempotency_key_idx
  ON withdrawals (merchant_id, idempotency_key)
  WHERE idempotency_key IS NOT NULL;
