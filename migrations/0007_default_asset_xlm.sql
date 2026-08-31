-- `payments.asset` and `payment_requests.asset` defaulted to 'cNGN', but every
-- deposit the worker actually records today is XLM (see
-- src/blockchain/worker.rs: process_deposit sets `asset` explicitly from
-- fetch_for_address's 'native' -> 'XLM' mapping), and the payment-requests API
-- handler already defaults new requests to 'XLM', not this column's default.
-- Both INSERTs always supply `asset` explicitly, so this default was never
-- actually hit in practice — it only misled a reader of the schema into
-- thinking cNGN is the primary asset today.
--
-- `withdrawals.asset` keeps its 'cNGN' default: create_withdrawal() genuinely
-- only supports cNGN today (WithdrawalError::UnsupportedAsset otherwise), so
-- that default reflects reality and is left alone.

ALTER TABLE payments ALTER COLUMN asset SET DEFAULT 'XLM';
ALTER TABLE payment_requests ALTER COLUMN asset SET DEFAULT 'XLM';

COMMENT ON COLUMN payments.asset IS
  'Defaults to XLM: the only deposit path implemented today (per-wallet Stellar polling) only ever records XLM. cNGN is the eventual primary asset once a cNGN issuer/anchor path exists — see PRD.';

COMMENT ON COLUMN payment_requests.asset IS
  'Defaults to XLM to match the API handler default and what is actually scannable/payable today. cNGN is the eventual primary asset once a cNGN issuer address is configured — see PRD.';
