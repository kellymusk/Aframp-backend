-- Automated Regulatory Report Generation & Compliance Filings Pipeline
-- Issue #523

-- ── Regulatory reports ────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS regulatory_reports (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    report_type             TEXT NOT NULL CHECK (report_type IN ('CTR','SAR','LIQUIDITY_RATIO','CROSS_BORDER_FLOW')),
    tenant_id               UUID,
    agency_tag              TEXT NOT NULL,
    submission_tracking_no  TEXT UNIQUE,
    payload_hash            TEXT NOT NULL,
    schema_format           TEXT NOT NULL DEFAULT 'XML' CHECK (schema_format IN ('XBRL','XML','JSON')),
    status                  TEXT NOT NULL DEFAULT 'PENDING'
                                CHECK (status IN ('PENDING','COMPILED','TRANSMITTED','FILED','FAILED_REMISSION','PENDING_RETRY')),
    compiled_at             TIMESTAMPTZ,
    transmitted_at          TIMESTAMPTZ,
    filed_at                TIMESTAMPTZ,
    next_retry_at           TIMESTAMPTZ,
    retry_count             INT NOT NULL DEFAULT 0,
    error_detail            TEXT,
    payload_bytes           BIGINT,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_regulatory_reports_type_status ON regulatory_reports(report_type, status);
CREATE INDEX IF NOT EXISTS idx_regulatory_reports_tenant       ON regulatory_reports(tenant_id);
CREATE INDEX IF NOT EXISTS idx_regulatory_reports_created      ON regulatory_reports(created_at DESC);

-- ── Agency gateways ───────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS agency_gateways (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agency_tag          TEXT NOT NULL UNIQUE,
    display_name        TEXT NOT NULL,
    jurisdiction        TEXT NOT NULL,
    endpoint_url        TEXT NOT NULL,
    protocol            TEXT NOT NULL CHECK (protocol IN ('REST','SFTP','WEBSOCKET','GRPC')),
    auth_method         TEXT NOT NULL CHECK (auth_method IN ('OAUTH2','API_KEY','MTLS','NONE')),
    credentials_ref     TEXT,
    public_key_pem      TEXT,
    tls_version         TEXT NOT NULL DEFAULT 'TLS1.3',
    is_active           BOOLEAN NOT NULL DEFAULT TRUE,
    last_ping_at        TIMESTAMPTZ,
    last_ping_status    TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_agency_gateways_active ON agency_gateways(is_active);

-- ── Audit filing history (time-series, immutable) ────────────────────────────

CREATE TABLE IF NOT EXISTS audit_filing_history (
    id                  UUID NOT NULL DEFAULT gen_random_uuid(),
    regulatory_report_id UUID NOT NULL REFERENCES regulatory_reports(id) ON DELETE CASCADE,
    gateway_id          UUID REFERENCES agency_gateways(id),
    event_type          TEXT NOT NULL CHECK (event_type IN ('COMPILED','TRANSMITTED','ACK','NACK','RETRY','ERROR')),
    http_status         INT,
    ack_code            TEXT,
    nack_reason         TEXT,
    payload_hash        TEXT,
    duration_ms         INT,
    actor               TEXT NOT NULL DEFAULT 'system',
    raw_response        JSONB,
    occurred_at         TIMESTAMPTZ NOT NULL DEFAULT now()
) PARTITION BY RANGE (occurred_at);

CREATE TABLE IF NOT EXISTS audit_filing_history_2027
    PARTITION OF audit_filing_history
    FOR VALUES FROM ('2027-01-01') TO ('2028-01-01');

CREATE TABLE IF NOT EXISTS audit_filing_history_2028
    PARTITION OF audit_filing_history
    FOR VALUES FROM ('2028-01-01') TO ('2029-01-01');

CREATE INDEX IF NOT EXISTS idx_afh_report_id ON audit_filing_history(regulatory_report_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS idx_afh_event_type ON audit_filing_history(event_type, occurred_at DESC);
