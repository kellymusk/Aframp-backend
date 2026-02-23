-- migrate:up
-- #5 Implement Database Indexes, Constraints, and Performance Optimization (AFRI extensions)
-- Purpose: Add indexes and constraints for AFRI conversion audits, trustline ops, fee structures,
--          and optimize exchange rate lookups.

-- ============================================================================
-- EXCHANGE RATE OPTIMIZATIONS
-- ============================================================================

-- Composite index for latest rate lookup by pair
CREATE INDEX IF NOT EXISTS idx_exchange_rates_pair_valid_until
ON exchange_rates(from_currency, to_currency, valid_until DESC NULLS LAST);

-- Ensure valid_until is not before valid_from when set
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'chk_exchange_rates_valid_range'
    ) THEN
        ALTER TABLE exchange_rates
        ADD CONSTRAINT chk_exchange_rates_valid_range
        CHECK (valid_until IS NULL OR valid_until >= valid_from);
    END IF;
END $$;

-- ============================================================================
-- CONVERSION AUDIT INDEXES
-- ============================================================================

CREATE INDEX IF NOT EXISTS idx_conversion_audits_user_created_at
ON conversion_audits(user_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_conversion_audits_wallet_created_at
ON conversion_audits(wallet_address, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_conversion_audits_status_created_at
ON conversion_audits(status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_conversion_audits_transaction_id
ON conversion_audits(transaction_id);

CREATE INDEX IF NOT EXISTS idx_conversion_audits_metadata_gin
ON conversion_audits USING GIN (metadata);

-- ============================================================================
-- TRUSTLINE OPERATIONS INDEXES
-- ============================================================================

CREATE INDEX IF NOT EXISTS idx_trustline_operations_wallet_created_at
ON trustline_operations(wallet_address, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_trustline_operations_status_created_at
ON trustline_operations(status, created_at DESC);

-- ============================================================================
-- FEE STRUCTURE INDEXES & CONSTRAINTS
-- ============================================================================

CREATE INDEX IF NOT EXISTS idx_fee_structures_active_type
ON fee_structures(fee_type, effective_from DESC)
WHERE is_active = TRUE;

CREATE INDEX IF NOT EXISTS idx_fee_structures_type_effective
ON fee_structures(fee_type, effective_from, effective_until);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'chk_fee_structures_min_max'
    ) THEN
        ALTER TABLE fee_structures
        ADD CONSTRAINT chk_fee_structures_min_max
        CHECK (min_fee IS NULL OR max_fee IS NULL OR min_fee <= max_fee);
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'chk_fee_structures_effective_range'
    ) THEN
        ALTER TABLE fee_structures
        ADD CONSTRAINT chk_fee_structures_effective_range
        CHECK (effective_until IS NULL OR effective_until >= effective_from);
    END IF;
END $$;

