-- Migration: AML/KYC Compliance Effectiveness Reporting System
-- Creates tables for automated compliance KPI tracking and report generation

-- Compliance effectiveness reports (generated on-demand or scheduled)
CREATE TABLE IF NOT EXISTS compliance_effectiveness_reports (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    report_type             TEXT NOT NULL CHECK (report_type IN ('monthly', 'quarterly', 'annual', 'ad_hoc')),
    period_start            TIMESTAMPTZ NOT NULL,
    period_end              TIMESTAMPTZ NOT NULL,
    
    -- Alert Volume Metrics
    total_alerts            INTEGER NOT NULL DEFAULT 0,
    sanctions_alerts        INTEGER NOT NULL DEFAULT 0,
    aml_alerts              INTEGER NOT NULL DEFAULT 0,
    kyc_alerts              INTEGER NOT NULL DEFAULT 0,
    
    -- False Positive Metrics
    false_positives         INTEGER NOT NULL DEFAULT 0,
    false_positive_rate     DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    
    -- Resolution SLA Metrics
    avg_resolution_time_hrs DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    median_resolution_time_hrs DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    sla_breaches            INTEGER NOT NULL DEFAULT 0,
    sla_compliance_rate     DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    
    -- Case Status Distribution
    cases_cleared           INTEGER NOT NULL DEFAULT 0,
    cases_blocked           INTEGER NOT NULL DEFAULT 0,
    cases_pending           INTEGER NOT NULL DEFAULT 0,
    
    -- Risk Score Distribution
    low_risk_cases          INTEGER NOT NULL DEFAULT 0,
    medium_risk_cases       INTEGER NOT NULL DEFAULT 0,
    critical_risk_cases     INTEGER NOT NULL DEFAULT 0,
    
    -- Trend Analysis
    alert_volume_trend      TEXT, -- 'increasing', 'decreasing', 'stable'
    false_positive_trend    TEXT,
    
    -- Report Metadata
    generated_by            TEXT NOT NULL,
    generated_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    format                  TEXT NOT NULL CHECK (format IN ('pdf', 'csv', 'json')),
    file_path               TEXT,
    
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Report schedules for automated generation
CREATE TABLE IF NOT EXISTS compliance_report_schedules (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    schedule_name       TEXT NOT NULL UNIQUE,
    report_type         TEXT NOT NULL CHECK (report_type IN ('monthly', 'quarterly', 'annual')),
    cron_expression     TEXT NOT NULL,
    format              TEXT NOT NULL CHECK (format IN ('pdf', 'csv', 'json')),
    recipients          TEXT[] NOT NULL DEFAULT '{}',
    enabled             BOOLEAN NOT NULL DEFAULT TRUE,
    last_run_at         TIMESTAMPTZ,
    next_run_at         TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Audit trail for report generation
CREATE TABLE IF NOT EXISTS compliance_report_audit (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    report_id           UUID NOT NULL REFERENCES compliance_effectiveness_reports(id) ON DELETE CASCADE,
    action              TEXT NOT NULL CHECK (action IN ('generated', 'downloaded', 'emailed', 'deleted')),
    actor_id            TEXT NOT NULL,
    actor_role          TEXT NOT NULL,
    actor_ip            TEXT,
    metadata            JSONB,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_compliance_reports_period ON compliance_effectiveness_reports(period_start, period_end);
CREATE INDEX IF NOT EXISTS idx_compliance_reports_type ON compliance_effectiveness_reports(report_type, generated_at DESC);
CREATE INDEX IF NOT EXISTS idx_compliance_reports_generated_by ON compliance_effectiveness_reports(generated_by, generated_at DESC);
CREATE INDEX IF NOT EXISTS idx_compliance_schedules_next_run ON compliance_report_schedules(next_run_at) WHERE enabled = TRUE;
CREATE INDEX IF NOT EXISTS idx_compliance_audit_report_id ON compliance_report_audit(report_id, created_at DESC);

-- Trigger: update updated_at
CREATE TRIGGER compliance_schedules_updated_at
    BEFORE UPDATE ON compliance_report_schedules
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

-- Seed default monthly schedule
INSERT INTO compliance_report_schedules (schedule_name, report_type, cron_expression, format, recipients)
VALUES
    ('monthly_compliance_report', 'monthly', '0 0 1 * *', 'pdf', ARRAY['compliance@aframp.io']),
    ('quarterly_compliance_report', 'quarterly', '0 0 1 1,4,7,10 *', 'pdf', ARRAY['compliance@aframp.io', 'cfo@aframp.io'])
ON CONFLICT (schedule_name) DO NOTHING;

COMMENT ON TABLE compliance_effectiveness_reports IS 'Automated AML/KYC compliance effectiveness reports with KPI tracking';
COMMENT ON TABLE compliance_report_schedules IS 'Scheduled report generation with cron-style triggers';
COMMENT ON TABLE compliance_report_audit IS 'Audit trail for compliance report access and distribution';
