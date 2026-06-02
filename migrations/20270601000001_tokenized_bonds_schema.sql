-- Programmable Sovereign Debt Settlement & Tokenized Treasury Bond Rails
-- Issue #524

-- ── Tokenized bond instruments ────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS tokenized_bond_instruments (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    isin                TEXT NOT NULL UNIQUE,
    issuer_authority    TEXT NOT NULL,
    instrument_name     TEXT NOT NULL,
    currency            TEXT NOT NULL DEFAULT 'NGN',
    face_value          NUMERIC(28, 7) NOT NULL,
    coupon_rate_bps     INT NOT NULL DEFAULT 0,
    maturity_at         TIMESTAMPTZ NOT NULL,
    auction_date        TIMESTAMPTZ,
    on_chain_asset_code TEXT,
    stellar_issuer      TEXT,
    status              TEXT NOT NULL DEFAULT 'ACTIVE'
                            CHECK (status IN ('ACTIVE','MATURED','REDEEMED','SUSPENDED')),
    metadata            JSONB NOT NULL DEFAULT '{}',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_tbi_status       ON tokenized_bond_instruments(status);
CREATE INDEX IF NOT EXISTS idx_tbi_maturity     ON tokenized_bond_instruments(maturity_at);

-- ── Bond ledger allocations ───────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS bond_ledger_allocations (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id               UUID NOT NULL,
    bond_instrument_id      UUID NOT NULL REFERENCES tokenized_bond_instruments(id) ON DELETE RESTRICT,
    fractional_units        NUMERIC(28, 7) NOT NULL CHECK (fractional_units > 0),
    purchase_price          NUMERIC(28, 7) NOT NULL,
    accrued_yield           NUMERIC(28, 7) NOT NULL DEFAULT 0,
    on_chain_token_hash     TEXT,
    stellar_tx_hash         TEXT,
    status                  TEXT NOT NULL DEFAULT 'ACTIVE'
                                CHECK (status IN ('ACTIVE','REDEEMED','LIQUIDATING','LIQUIDATED')),
    acquired_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    redeemed_at             TIMESTAMPTZ,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_bla_tenant       ON bond_ledger_allocations(tenant_id);
CREATE INDEX IF NOT EXISTS idx_bla_instrument   ON bond_ledger_allocations(bond_instrument_id);
CREATE INDEX IF NOT EXISTS idx_bla_status       ON bond_ledger_allocations(status);

-- ── Automated sweep policies ──────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS automated_sweep_policies (
    id                          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id                   UUID NOT NULL UNIQUE,
    enabled                     BOOLEAN NOT NULL DEFAULT TRUE,
    min_sweep_threshold_ngn     NUMERIC(28, 7) NOT NULL DEFAULT 1000000,
    max_portfolio_duration_days INT NOT NULL DEFAULT 90,
    preferred_instrument_id     UUID REFERENCES tokenized_bond_instruments(id),
    last_sweep_at               TIMESTAMPTZ,
    next_sweep_at               TIMESTAMPTZ,
    sweep_interval_minutes      INT NOT NULL DEFAULT 60,
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_asp_tenant       ON automated_sweep_policies(tenant_id);
CREATE INDEX IF NOT EXISTS idx_asp_next_sweep   ON automated_sweep_policies(next_sweep_at) WHERE enabled = TRUE;
