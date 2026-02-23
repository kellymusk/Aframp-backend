-- migrate:up
-- Foundational schema for Aframp core entities: users, wallets, transactions, AFRI trustlines.
-- Notes:
-- - Monetary values use NUMERIC (never FLOAT/DOUBLE).
-- - UUID primary keys are generated via pgcrypto's gen_random_uuid().
-- - Statuses are stored in a lookup table to keep them extensible.

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- Keep updated_at current on every UPDATE.
CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS TRIGGER AS $$
BEGIN
  NEW.updated_at = now();
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TABLE users (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  email TEXT NOT NULL UNIQUE,
  phone TEXT UNIQUE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE users IS 'Aframp users for non-custodial accounts.';
COMMENT ON COLUMN users.email IS 'Primary user identifier for non-custodial accounts.';
COMMENT ON COLUMN users.phone IS 'Optional phone number for user identification.';
COMMENT ON COLUMN users.created_at IS 'Timestamp when the user was created.';
COMMENT ON COLUMN users.updated_at IS 'Timestamp when the user was last updated.';

CREATE TABLE wallets (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  wallet_address VARCHAR(255) NOT NULL UNIQUE,
  account_address VARCHAR(255),
  chain TEXT NOT NULL CHECK (chain IN ('stellar', 'ethereum', 'bitcoin')),
  has_afri_trustline BOOLEAN NOT NULL DEFAULT FALSE,
  afri_balance NUMERIC(36, 18) NOT NULL DEFAULT 0,
  balance TEXT NOT NULL DEFAULT '0',
  last_balance_check TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE wallets IS 'Connected wallet addresses across supported chains.';
COMMENT ON COLUMN wallets.wallet_address IS 'Unique on-chain address; Stellar addresses are 56 chars but longer are supported.';
COMMENT ON COLUMN wallets.account_address IS 'Alternative account identifier for the wallet.';
COMMENT ON COLUMN wallets.chain IS 'Blockchain network identifier.';
COMMENT ON COLUMN wallets.afri_balance IS 'Cached AFRI balance for quick reads; refresh via last_balance_check.';
COMMENT ON COLUMN wallets.balance IS 'Current wallet balance as string.';
COMMENT ON COLUMN wallets.last_balance_check IS 'Timestamp of last on-chain AFRI balance refresh.';
COMMENT ON COLUMN wallets.created_at IS 'Timestamp when the wallet record was created.';
COMMENT ON COLUMN wallets.updated_at IS 'Timestamp when the wallet record was last updated.';

CREATE TABLE transaction_statuses (
  code TEXT PRIMARY KEY,
  description TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE transaction_statuses IS 'Lookup table to allow extensible transaction statuses.';
COMMENT ON COLUMN transaction_statuses.code IS 'Machine-readable status code.';
COMMENT ON COLUMN transaction_statuses.description IS 'Human-readable status description.';
COMMENT ON COLUMN transaction_statuses.created_at IS 'Timestamp when the status was created.';
COMMENT ON COLUMN transaction_statuses.updated_at IS 'Timestamp when the status was last updated.';

INSERT INTO transaction_statuses (code, description) VALUES
  ('pending', 'Awaiting processing'),
  ('processing', 'Processing in provider or blockchain'),
  ('completed', 'Completed successfully'),
  ('failed', 'Failed or reverted');

CREATE TABLE transactions (
  transaction_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  wallet_address VARCHAR(255) NOT NULL
    REFERENCES wallets(wallet_address) ON UPDATE CASCADE ON DELETE RESTRICT,
  type TEXT NOT NULL CHECK (type IN ('onramp', 'offramp', 'bill_payment')),
  from_currency TEXT NOT NULL,
  to_currency TEXT NOT NULL,
  from_amount NUMERIC(36, 18) NOT NULL CHECK (from_amount >= 0),
  to_amount NUMERIC(36, 18) NOT NULL CHECK (to_amount >= 0),
  afri_amount NUMERIC(36, 18) NOT NULL DEFAULT 0 CHECK (afri_amount >= 0),
  status TEXT NOT NULL REFERENCES transaction_statuses(code),
  payment_provider TEXT CHECK (payment_provider IN ('flutterwave', 'paystack', 'mpesa')),
  payment_reference TEXT,
  blockchain_tx_hash TEXT,
  error_message TEXT,
  metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE transactions IS 'All onramp/offramp/bill payment operations.';
COMMENT ON COLUMN transactions.type IS 'Operation type such as onramp, offramp, or bill payment.';
COMMENT ON COLUMN transactions.from_currency IS 'Currency the user pays with (fiat/crypto).';
COMMENT ON COLUMN transactions.to_currency IS 'Currency the user receives (fiat/crypto).';
COMMENT ON COLUMN transactions.from_amount IS 'Amount paid in from_currency.';
COMMENT ON COLUMN transactions.to_amount IS 'Amount received in to_currency.';
COMMENT ON COLUMN transactions.afri_amount IS 'AFRI stablecoin amount minted or redeemed in this transaction.';
COMMENT ON COLUMN transactions.status IS 'Extensible status code referencing transaction_statuses.';
COMMENT ON COLUMN transactions.payment_provider IS 'Payment rail/provider used for fiat leg.';
COMMENT ON COLUMN transactions.payment_reference IS 'Provider reference or receipt identifier.';
COMMENT ON COLUMN transactions.blockchain_tx_hash IS 'On-chain transaction hash when applicable.';
COMMENT ON COLUMN transactions.error_message IS 'Failure reason if status is failed.';
COMMENT ON COLUMN transactions.metadata IS 'Provider-specific data payload.';
COMMENT ON COLUMN transactions.created_at IS 'Timestamp when the transaction was created.';
COMMENT ON COLUMN transactions.updated_at IS 'Timestamp when the transaction was last updated.';

CREATE TABLE afri_trustlines (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  wallet_address VARCHAR(255) NOT NULL UNIQUE
    REFERENCES wallets(wallet_address) ON UPDATE CASCADE ON DELETE CASCADE,
  established_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE exchange_rates (
  id TEXT PRIMARY KEY,
  from_currency TEXT NOT NULL,
  to_currency TEXT NOT NULL,
  rate TEXT NOT NULL,
  source TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (from_currency, to_currency)
);

CREATE TABLE trustlines (
  id TEXT PRIMARY KEY,
  account VARCHAR(255) NOT NULL,
  asset_code TEXT NOT NULL,
  balance TEXT NOT NULL DEFAULT '0',
  "limit" TEXT NOT NULL DEFAULT '0',
  issuer TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'active', 'inactive', 'deleted')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (account, asset_code)
);

COMMENT ON TABLE afri_trustlines IS 'AFRI trustline establishment per wallet.';
COMMENT ON COLUMN afri_trustlines.established_at IS 'Timestamp when the trustline was established.';
COMMENT ON COLUMN afri_trustlines.metadata IS 'Chain-specific trustline metadata.';
COMMENT ON COLUMN afri_trustlines.created_at IS 'Timestamp when the trustline record was created.';
COMMENT ON COLUMN afri_trustlines.updated_at IS 'Timestamp when the trustline record was last updated.';

COMMENT ON TABLE exchange_rates IS 'Current and historical exchange rates between currencies.';
COMMENT ON COLUMN exchange_rates.id IS 'Unique identifier for the exchange rate record.';
COMMENT ON COLUMN exchange_rates.from_currency IS 'Source currency code.';
COMMENT ON COLUMN exchange_rates.to_currency IS 'Target currency code.';
COMMENT ON COLUMN exchange_rates.rate IS 'Exchange rate as string.';
COMMENT ON COLUMN exchange_rates.source IS 'Source of the rate (e.g., external API, manual input).';
COMMENT ON COLUMN exchange_rates.created_at IS 'Timestamp when the rate was created.';
COMMENT ON COLUMN exchange_rates.updated_at IS 'Timestamp when the rate was last updated.';

COMMENT ON TABLE trustlines IS 'Trustline operations for asset tracking.';
COMMENT ON COLUMN trustlines.id IS 'Unique identifier for the trustline.';
COMMENT ON COLUMN trustlines.account IS 'Account address for the trustline.';
COMMENT ON COLUMN trustlines.asset_code IS 'Asset code for the trustline.';
COMMENT ON COLUMN trustlines.balance IS 'Current trustline balance as string.';
COMMENT ON COLUMN trustlines."limit" IS 'Trustline limit as string.';
COMMENT ON COLUMN trustlines.issuer IS 'Issuer of the asset.';
COMMENT ON COLUMN trustlines.status IS 'Current status of the trustline.';
COMMENT ON COLUMN trustlines.created_at IS 'Timestamp when the trustline was created.';
COMMENT ON COLUMN trustlines.updated_at IS 'Timestamp when the trustline was last updated.';

CREATE TRIGGER set_updated_at_users
  BEFORE UPDATE ON users
  FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER set_updated_at_wallets
  BEFORE UPDATE ON wallets
  FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER set_updated_at_transaction_statuses
  BEFORE UPDATE ON transaction_statuses
  FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER set_updated_at_transactions
  BEFORE UPDATE ON transactions
  FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER set_updated_at_afri_trustlines
  BEFORE UPDATE ON afri_trustlines
  FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER set_updated_at_exchange_rates
  BEFORE UPDATE ON exchange_rates
  FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER set_updated_at_trustlines
  BEFORE UPDATE ON trustlines
  FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Indexes for frequently queried columns.
-- Note: users.email and wallets.wallet_address are already indexed via UNIQUE constraints.
CREATE INDEX idx_transactions_wallet_address ON transactions(wallet_address);
CREATE INDEX idx_transactions_status ON transactions(status);
CREATE INDEX idx_wallets_account_address ON wallets(account_address);
CREATE INDEX idx_exchange_rates_currencies ON exchange_rates(from_currency, to_currency);
CREATE INDEX idx_trustlines_account ON trustlines(account);
CREATE INDEX idx_trustlines_asset ON trustlines(asset_code);

