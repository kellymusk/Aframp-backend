-- Automated Central Bank Clearing & Interbank Settlement Rail (RTGS Bridge)
-- Issue #525

-- ── RTGS settlement pools ─────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS rtgs_settlement_pools (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bank_code               TEXT NOT NULL UNIQUE,
    bank_name               TEXT NOT NULL,
    currency                TEXT NOT NULL DEFAULT 'NGN',
    available_limit         NUMERIC(28, 7) NOT NULL DEFAULT 0,
    net_debit_cap           NUMERIC(28, 7) NOT NULL DEFAULT 0,
    clearing_account_ref    TEXT NOT NULL,
    is_active               BOOLEAN NOT NULL DEFAULT TRUE,
    last_settlement_at      TIMESTAMPTZ,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_rtgs_pools_active ON rtgs_settlement_pools(is_active);

-- ── Clearing house ledger entries ─────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS clearing_house_ledger_entries (
    id                          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    settlement_pool_id          UUID NOT NULL REFERENCES rtgs_settlement_pools(id) ON DELETE RESTRICT,
    on_chain_tx_hash            TEXT,
    stellar_ledger_sequence     BIGINT,
    bank_tracking_ref           TEXT NOT NULL UNIQUE,
    amount                      NUMERIC(28, 7) NOT NULL,
    currency                    TEXT NOT NULL DEFAULT 'NGN',
    direction                   TEXT NOT NULL CHECK (direction IN ('INBOUND','OUTBOUND')),
    status                      TEXT NOT NULL DEFAULT 'PENDING'
                                    CHECK (status IN ('PENDING','SETTLED','REVERSED','HELD_FOR_RECONCILIATION','FAILED')),
    two_pc_phase                TEXT NOT NULL DEFAULT 'NONE'
                                    CHECK (two_pc_phase IN ('NONE','PREPARE','COMMIT','ABORT')),
    hsm_signature               TEXT,
    aml_metadata                JSONB NOT NULL DEFAULT '{}',
    settled_at                  TIMESTAMPTZ,
    reversed_at                 TIMESTAMPTZ,
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_chle_pool_id  ON clearing_house_ledger_entries(settlement_pool_id);
CREATE INDEX IF NOT EXISTS idx_chle_status   ON clearing_house_ledger_entries(status);
CREATE INDEX IF NOT EXISTS idx_chle_ref      ON clearing_house_ledger_entries(bank_tracking_ref);

-- ── Interbank reconciliation logs (time-series, immutable) ───────────────────

CREATE TABLE IF NOT EXISTS interbank_reconciliation_logs (
    id                  UUID NOT NULL DEFAULT gen_random_uuid(),
    ledger_entry_id     UUID NOT NULL REFERENCES clearing_house_ledger_entries(id) ON DELETE CASCADE,
    ack_code            TEXT,
    nack_reason         TEXT,
    message_type        TEXT NOT NULL,
    iso20022_payload    JSONB,
    processing_node     TEXT,
    duration_ms         INT,
    occurred_at         TIMESTAMPTZ NOT NULL DEFAULT now()
) PARTITION BY RANGE (occurred_at);

CREATE TABLE IF NOT EXISTS interbank_reconciliation_logs_2027
    PARTITION OF interbank_reconciliation_logs
    FOR VALUES FROM ('2027-01-01') TO ('2028-01-01');

CREATE TABLE IF NOT EXISTS interbank_reconciliation_logs_2028
    PARTITION OF interbank_reconciliation_logs
    FOR VALUES FROM ('2028-01-01') TO ('2029-01-01');

CREATE INDEX IF NOT EXISTS idx_irl_entry_id  ON interbank_reconciliation_logs(ledger_entry_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS idx_irl_occurred  ON interbank_reconciliation_logs(occurred_at DESC);
