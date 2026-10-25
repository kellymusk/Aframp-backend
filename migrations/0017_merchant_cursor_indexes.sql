-- Performance (#1220): keyset ("cursor") pagination for merchant transaction
-- lists. The functions payments_by_merchant_cursor,
-- withdrawals_by_merchant_cursor and payment_requests_by_merchant_cursor all
-- run the shape:
--
--   WHERE merchant_id = $1 AND (created_at, id) < ($2, $3)
--   ORDER BY created_at DESC, id DESC
--   LIMIT $4
--
-- 0009_admin_list_indexes.sql added (merchant_id, created_at DESC) indexes,
-- which cover the equality filter but force Postgres to sort/heap-fetch to
-- resolve the `id` tie-break. These composite indexes add `id DESC` as the
-- final key so the whole row-comparison is satisfied by one backward index
-- range scan.
--
-- Expected plan (EXPLAIN ANALYZE, merchant with N payments):
--   Index Scan Backward using payments_merchant_cursor_idx on payments
--     Index Cond: (merchant_id = $1 AND (ROW(created_at, id) < ROW($2, $3)))
--   -- no Seq Scan, no Sort; cost scales with the page size, not N.
-- Before this index the same query degenerates to a Seq Scan + Sort over all
-- of the merchant's rows.

CREATE INDEX IF NOT EXISTS payments_merchant_cursor_idx
    ON payments (merchant_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS withdrawals_merchant_cursor_idx
    ON withdrawals (merchant_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS payment_requests_merchant_cursor_idx
    ON payment_requests (merchant_id, created_at DESC, id DESC);
