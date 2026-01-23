-- ============================================================================
-- 1. EXCHANGE_RATES TABLE
-- ============================================================================
CREATE TABLE exchange_rates (
    id              BIGSERIAL PRIMARY KEY,
    currency_pair   VARCHAR(20)     NOT NULL,
    rate            DECIMAL(18,8)   NOT NULL CHECK (rate > 0),
    spread          DECIMAL(6,4)    DEFAULT 0.0000,
    source          VARCHAR(50)     NOT NULL,
    valid_from      TIMESTAMPTZ     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    valid_until     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at      TIMESTAMPTZ     DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT chk_valid_dates CHECK (valid_until IS NULL OR valid_until > valid_from)
);

CREATE INDEX idx_exchange_rates_pair_valid
    ON exchange_rates (currency_pair, valid_from DESC);

CREATE INDEX idx_exchange_rates_current
    ON exchange_rates (currency_pair)
    WHERE valid_until IS NULL;

-- Note: Partitioning considered — enable later when volume grows
-- Example (uncomment when needed):
/*
ALTER TABLE exchange_rates
    SET (partitioning = RANGE (valid_from));

CREATE TABLE exchange_rates_y2025 PARTITION OF exchange_rates
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
*/

-- ============================================================================
-- 2. AFRI_CONVERSIONS TABLE
-- ============================================================================
CREATE TABLE afri_conversions (
    conversion_id     BIGSERIAL PRIMARY KEY,
    transaction_id    BIGINT          NOT NULL,
    from_currency     VARCHAR(10)     NOT NULL,
    to_currency       VARCHAR(10)     NOT NULL,
    from_amount       DECIMAL(18,8)   NOT NULL CHECK (from_amount > 0),
    to_amount         DECIMAL(18,8)   NOT NULL CHECK (to_amount > 0),
    exchange_rate_used DECIMAL(18,8)  NOT NULL CHECK (exchange_rate_used > 0),
    timestamp         TIMESTAMPTZ     NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT chk_different_currencies CHECK (from_currency != to_currency)
);

CREATE INDEX idx_afri_conversions_tx
    ON afri_conversions(transaction_id);

CREATE INDEX idx_afri_conversions_time
    ON afri_conversions(timestamp DESC);

CREATE INDEX idx_afri_conversions_currencies
    ON afri_conversions(from_currency, to_currency, timestamp DESC);

-- Optional FK (uncomment when transactions table exists)
-- ALTER TABLE afri_conversions
--     ADD CONSTRAINT fk_conversion_transaction
--     FOREIGN KEY (transaction_id) REFERENCES transactions(id);

-- ============================================================================
-- 3. TRUSTLINE_OPERATIONS TABLE
-- ============================================================================
CREATE TABLE trustline_operations (
    id              BIGSERIAL PRIMARY KEY,
    wallet_address  VARCHAR(56)     NOT NULL,
    stellar_tx_hash VARCHAR(64),
    status          VARCHAR(20)     NOT NULL
        CHECK (status IN ('pending', 'confirmed', 'failed')),
    error_message   TEXT,
    retry_count     INTEGER         NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    created_at      TIMESTAMPTZ     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at      TIMESTAMPTZ     DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_trustline_wallet
    ON trustline_operations(wallet_address);

CREATE INDEX idx_trustline_status_time
    ON trustline_operations(status, created_at DESC);

CREATE UNIQUE INDEX idx_trustline_pending_unique
    ON trustline_operations(wallet_address)
    WHERE status = 'pending';


-- ============================================================================
-- 4. FEE_STRUCTURES TABLE (supports country-specific & tiered fees)
-- ============================================================================

CREATE TABLE fee_structures (
    id              BIGSERIAL PRIMARY KEY,
    transaction_type VARCHAR(30)     NOT NULL,
    min_amount      DECIMAL(18,8)   NOT NULL DEFAULT 0,
    max_amount      DECIMAL(18,8),
    fee_percentage  DECIMAL(6,4)    NOT NULL,
    flat_fee_amount DECIMAL(18,8)   NOT NULL DEFAULT 0,
    currency        VARCHAR(10)     NOT NULL,
    country_code    CHAR(3),
    is_active       BOOLEAN         NOT NULL DEFAULT true,
    effective_from  TIMESTAMPTZ     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    effective_until TIMESTAMPTZ,

    CONSTRAINT chk_fee_range     CHECK (max_amount IS NULL OR max_amount > min_amount),
    CONSTRAINT chk_fee_non_neg   CHECK (fee_percentage >= 0 AND flat_fee_amount >= 0)
);

CREATE INDEX idx_fee_active_type_currency
    ON fee_structures(transaction_type, currency)
    WHERE is_active = true;

CREATE INDEX idx_fee_country_type
    ON fee_structures(country_code, transaction_type)
    WHERE country_code IS NOT NULL AND is_active = true;


-- ============================================================================
-- Helper: Get current rate (required for efficient current-rate lookup)
-- ============================================================================
CREATE OR REPLACE FUNCTION get_current_afri_rate(p_pair VARCHAR)
RETURNS DECIMAL(18,8)
LANGUAGE sql
STABLE
AS $$
    SELECT rate
      FROM exchange_rates
     WHERE currency_pair = p_pair
       AND valid_until IS NULL
     ORDER BY valid_from DESC
     LIMIT 1;
$$;

-- ============================================================================
-- Helper: Insert new rate with automatic TTL/expiry (covers expiry note)
-- ============================================================================
CREATE OR REPLACE PROCEDURE insert_rate_with_ttl(
    p_pair      VARCHAR,
    p_rate      DECIMAL(18,8),
    p_spread    DECIMAL(6,4) DEFAULT 0,
    p_source    VARCHAR DEFAULT 'api',
    p_ttl_hours INTEGER     DEFAULT 24
)
LANGUAGE plpgsql
AS $$
BEGIN
    UPDATE exchange_rates
       SET valid_until = CURRENT_TIMESTAMP
     WHERE currency_pair = p_pair
       AND valid_until IS NULL;

    INSERT INTO exchange_rates (
        currency_pair, rate, spread, source,
        valid_from, valid_until
    ) VALUES (
        p_pair, p_rate, p_spread, p_source,
        CURRENT_TIMESTAMP,
        CURRENT_TIMESTAMP + (p_ttl_hours || ' hours')::interval
    );
END;
$$;

-- ============================================================================
-- Optional: Sample data (uncomment when you want to seed the DB for testing)
-- ============================================================================
/*
INSERT INTO exchange_rates (currency_pair, rate, spread, source, valid_from) VALUES
    ('NGN/AFRI', 0.00068900, 0.0050, 'api', '2025-01-01 00:00:00'),
    ('KES/AFRI', 0.00738000, 0.0040, 'api', '2025-01-01 00:00:00'),
    ('GHS/AFRI', 0.07810000, 0.0060, 'api', '2025-01-01 00:00:00');

INSERT INTO fee_structures (transaction_type, min_amount, max_amount, fee_percentage, flat_fee_amount, currency, country_code) VALUES
    ('conversion', 0,    1000,   1.5000, 0, 'AFRI', NULL),
    ('conversion', 1000, NULL,   0.7500, 0, 'AFRI', NULL),
    ('withdrawal', 0,    NULL,   1.0000, 50, 'NGN', 'NGA');
*/