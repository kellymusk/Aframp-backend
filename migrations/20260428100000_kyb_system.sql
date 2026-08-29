-- Migration: KYB (Know Your Business) System
-- Corporate entity verification with registry integration, UBO extraction, and risk scoring

-- KYB applications (main entity verification workflow)
CREATE TABLE IF NOT EXISTS kyb_applications (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_id             UUID NOT NULL UNIQUE,
    business_name           TEXT NOT NULL,
    registration_number     TEXT NOT NULL,
    jurisdiction            TEXT NOT NULL, -- ISO 3166-1 alpha-2 (e.g., 'NG')
    industry_code           TEXT, -- NAICS or local equivalent
    registered_address      TEXT,
    
    -- Workflow state
    status                  TEXT NOT NULL DEFAULT 'draft'
                                CHECK (status IN ('draft', 'documents_submitted', 'registry_verified', 
                                                  'compliance_review', 'approved', 'rejected')),
    
    -- Registry verification
    registry_status         TEXT, -- 'active', 'inactive', 'deregistered'
    registry_verified_at    TIMESTAMPTZ,
    registry_data           JSONB,
    
    -- Risk assessment
    risk_level              TEXT CHECK (risk_level IN ('light', 'enhanced')),
    risk_score              DOUBLE PRECISION,
    
    -- Compliance review
    reviewed_by             TEXT,
    review_notes            TEXT,
    rejection_reason        TEXT,
    
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    approved_at             TIMESTAMPTZ
);

-- Ultimate Beneficial Owners (UBOs) - individuals owning >= 25%
CREATE TABLE IF NOT EXISTS kyb_ubos (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kyb_application_id      UUID NOT NULL REFERENCES kyb_applications(id) ON DELETE CASCADE,
    full_name               TEXT NOT NULL,
    ownership_percentage    DOUBLE PRECISION NOT NULL CHECK (ownership_percentage >= 0 AND ownership_percentage <= 100),
    nationality             TEXT, -- ISO 3166-1 alpha-2
    date_of_birth           DATE,
    
    -- KYC linkage
    kyc_user_id             UUID, -- Links to individual KYC if triggered
    kyc_status              TEXT, -- 'pending', 'verified', 'failed'
    
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Business documents (encrypted at rest)
CREATE TABLE IF NOT EXISTS kyb_documents (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kyb_application_id      UUID NOT NULL REFERENCES kyb_applications(id) ON DELETE CASCADE,
    document_type           TEXT NOT NULL CHECK (document_type IN ('memorandum', 'articles', 'proof_of_address', 
                                                                    'tax_certificate', 'bank_statement', 'other')),
    file_name               TEXT NOT NULL,
    file_path               TEXT NOT NULL, -- Encrypted storage path
    file_hash               TEXT NOT NULL, -- SHA-256 for integrity
    encrypted               BOOLEAN NOT NULL DEFAULT TRUE,
    
    -- OCR extraction
    ocr_extracted_data      JSONB,
    ocr_confidence          DOUBLE PRECISION,
    
    uploaded_at             TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Corporate registry checks (audit trail)
CREATE TABLE IF NOT EXISTS kyb_registry_checks (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kyb_application_id      UUID NOT NULL REFERENCES kyb_applications(id) ON DELETE CASCADE,
    registry_provider       TEXT NOT NULL, -- 'cac_nigeria', 'companies_house_uk', etc.
    registration_number     TEXT NOT NULL,
    check_status            TEXT NOT NULL, -- 'success', 'failed', 'not_found'
    response_data           JSONB,
    error_message           TEXT,
    checked_at              TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Risk scoring history
CREATE TABLE IF NOT EXISTS kyb_risk_scores (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kyb_application_id      UUID NOT NULL REFERENCES kyb_applications(id) ON DELETE CASCADE,
    score                   DOUBLE PRECISION NOT NULL CHECK (score >= 0 AND score <= 100),
    risk_level              TEXT NOT NULL CHECK (risk_level IN ('light', 'enhanced')),
    factors                 JSONB NOT NULL, -- { "industry_risk": 30, "jurisdiction_risk": 20, ... }
    calculated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_kyb_applications_merchant_id ON kyb_applications(merchant_id);
CREATE INDEX IF NOT EXISTS idx_kyb_applications_status ON kyb_applications(status);
CREATE INDEX IF NOT EXISTS idx_kyb_applications_registry_number ON kyb_applications(registration_number, jurisdiction);
CREATE INDEX IF NOT EXISTS idx_kyb_ubos_application_id ON kyb_ubos(kyb_application_id);
CREATE INDEX IF NOT EXISTS idx_kyb_ubos_kyc_status ON kyb_ubos(kyc_status) WHERE kyc_status = 'pending';
CREATE INDEX IF NOT EXISTS idx_kyb_documents_application_id ON kyb_documents(kyb_application_id);
CREATE INDEX IF NOT EXISTS idx_kyb_registry_checks_application_id ON kyb_registry_checks(kyb_application_id);

-- Trigger: update updated_at
CREATE TRIGGER kyb_applications_updated_at
    BEFORE UPDATE ON kyb_applications
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

COMMENT ON TABLE kyb_applications IS 'KYB applications for merchant onboarding with corporate registry verification';
COMMENT ON TABLE kyb_ubos IS 'Ultimate Beneficial Owners (>=25% ownership) requiring individual KYC';
COMMENT ON TABLE kyb_documents IS 'Encrypted business documents with OCR extraction';
COMMENT ON TABLE kyb_registry_checks IS 'Audit trail of corporate registry API calls';
COMMENT ON TABLE kyb_risk_scores IS 'Risk scoring history for compliance review';
