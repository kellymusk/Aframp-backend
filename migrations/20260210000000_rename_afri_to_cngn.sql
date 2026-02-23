-- migrate:up
-- Rename Afri-specific schema elements to CNGN to reflect the new stablecoin.
-- This migration updates existing tables, columns, indexes, and triggers.

-- 1. Wallet and transaction columns
ALTER TABLE wallets RENAME COLUMN afri_balance TO cngn_balance;
COMMENT ON COLUMN wallets.cngn_balance IS 'Cached cNGN balance for quick reads; refresh via last_balance_check.';

-- rename flag indicating trustline presence
ALTER TABLE wallets RENAME COLUMN has_afri_trustline TO has_cngn_trustline;
COMMENT ON COLUMN wallets.has_cngn_trustline IS 'Whether cNGN trustline exists';

ALTER TABLE transactions RENAME COLUMN afri_amount TO cngn_amount;
COMMENT ON COLUMN transactions.cngn_amount IS 'cNGN stablecoin amount minted or redeemed in this transaction.';

-- 2. Trustline table rename
ALTER TABLE afri_trustlines RENAME TO cngn_trustlines;
COMMENT ON TABLE cngn_trustlines IS 'cNGN trustline establishment per wallet.';

-- 3. Rename related indexes and triggers
ALTER INDEX IF EXISTS idx_afri_trustlines_created_at RENAME TO idx_cngn_trustlines_created_at;

DROP TRIGGER IF EXISTS set_updated_at_afri_trustlines ON cngn_trustlines;
CREATE TRIGGER set_updated_at_cngn_trustlines
  BEFORE UPDATE ON cngn_trustlines
  FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- 4. Update any foreign key constraints (none additional needed since wallet_address references unaffected)

