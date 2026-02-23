-- migrate:up
-- #4 Create AFRI Stablecoin Operations and Rates Schema
-- Purpose: Add exchange rate tracking enhancements, conversion audit trail,
--          trustline operations logging, and dynamic fee structures.

-- 1. Exchange rate tracking enhancements
ALTER TABLE exchange_rates
    ADD COLUMN IF NOT EXISTS valid_from TIMESTAMPTZ NOT NULL DEFAULT now(),
    ADD COLUMN IF NOT EXISTS valid_until TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_exchange_rates_currency_pair
    ON exchange_rates(from_currency, to_currency);

CREATE INDEX IF NOT EXISTS idx_exchange_rates_valid_until
    ON exchange_rates(valid_until);

COMMENT ON COLUMN exchange_rates.valid_from IS 'Timestamp when this rate became effective.';
COMMENT ON COLUMN exchange_rates.valid_until IS 'Timestamp when this rate expires (optional).';

-- 2. Conversion audit trail
CREATE TABLE IF NOT EXISTS conversion_audits (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    wallet_address VARCHAR(255) REFERENCES wallets(wallet_address) ON UPDATE CASCADE ON DELETE SET NULL,
    transaction_id UUID REFERENCES transactions(transaction_id) ON DELETE SET NULL,
    from_currency TEXT NOT NULL,
    to_currency TEXT NOT NULL,
    from_amount NUMERIC(36, 18) NOT NULL CHECK (from_amount >= 0),
    to_amount NUMERIC(36, 18) NOT NULL CHECK (to_amount >= 0),
    rate NUMERIC(36, 18) NOT NULL CHECK (rate > 0),
    fee_amount NUMERIC(36, 18) NOT NULL DEFAULT 0 CHECK (fee_amount >= 0),
    fee_currency TEXT,
    provider TEXT,
    status TEXT NOT NULL CHECK (status IN ('quoted', 'executed', 'failed', 'expired')),
    error_message TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE conversion_audits IS 'Audit trail for AFRI conversion quotes and executions.';
COMMENT ON COLUMN conversion_audits.rate IS 'Exchange rate used for the conversion.';
COMMENT ON COLUMN conversion_audits.status IS 'Conversion lifecycle status.';

CREATE INDEX IF NOT EXISTS idx_conversion_audits_user_id
    ON conversion_audits(user_id);
CREATE INDEX IF NOT EXISTS idx_conversion_audits_wallet
    ON conversion_audits(wallet_address);
CREATE INDEX IF NOT EXISTS idx_conversion_audits_transaction
    ON conversion_audits(transaction_id);
CREATE INDEX IF NOT EXISTS idx_conversion_audits_status
    ON conversion_audits(status);
CREATE INDEX IF NOT EXISTS idx_conversion_audits_created_at
    ON conversion_audits(created_at DESC);

-- 3. Trustline operations tracking
CREATE TABLE IF NOT EXISTS trustline_operations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    wallet_address VARCHAR(255) NOT NULL REFERENCES wallets(wallet_address) ON UPDATE CASCADE ON DELETE CASCADE,
    asset_code TEXT NOT NULL,
    issuer TEXT,
    operation_type TEXT NOT NULL CHECK (operation_type IN ('create', 'update', 'remove', 'verify')),
    status TEXT NOT NULL CHECK (status IN ('pending', 'completed', 'failed')),
    transaction_hash TEXT,
    error_message TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE trustline_operations IS 'Operational log for AFRI trustline lifecycle events.';
COMMENT ON COLUMN trustline_operations.operation_type IS 'Type of trustline operation performed.';
COMMENT ON COLUMN trustline_operations.status IS 'Operation status.';

CREATE INDEX IF NOT EXISTS idx_trustline_operations_wallet
    ON trustline_operations(wallet_address, status);
CREATE INDEX IF NOT EXISTS idx_trustline_operations_created_at
    ON trustline_operations(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_trustline_operations_asset
    ON trustline_operations(asset_code);

-- 4. Dynamic fee structures
CREATE TABLE IF NOT EXISTS fee_structures (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    fee_type TEXT NOT NULL CHECK (fee_type IN ('onramp', 'offramp', 'bill_payment', 'exchange', 'transfer')),
    fee_rate_bps INTEGER NOT NULL DEFAULT 0 CHECK (fee_rate_bps >= 0 AND fee_rate_bps <= 10000),
    fee_flat NUMERIC(36, 18) NOT NULL DEFAULT 0 CHECK (fee_flat >= 0),
    min_fee NUMERIC(36, 18),
    max_fee NUMERIC(36, 18),
    currency TEXT,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    effective_from TIMESTAMPTZ NOT NULL DEFAULT now(),
    effective_until TIMESTAMPTZ,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE fee_structures IS 'Configurable fee definitions for AFRI operations.';
COMMENT ON COLUMN fee_structures.fee_rate_bps IS 'Percentage fee rate expressed in basis points.';
COMMENT ON COLUMN fee_structures.fee_flat IS 'Flat fee amount applied to the operation.';

CREATE INDEX IF NOT EXISTS idx_fee_structures_active
    ON fee_structures(is_active, fee_type);
CREATE INDEX IF NOT EXISTS idx_fee_structures_effective
    ON fee_structures(effective_from, effective_until);

-- Triggers to maintain updated_at
CREATE TRIGGER set_updated_at_conversion_audits
    BEFORE UPDATE ON conversion_audits
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER set_updated_at_trustline_operations
    BEFORE UPDATE ON trustline_operations
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER set_updated_at_fee_structures
    BEFORE UPDATE ON fee_structures
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

