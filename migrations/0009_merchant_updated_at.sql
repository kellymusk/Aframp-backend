-- Migration: add updated_at to merchants table with auto-update trigger
-- Issue #1073: missing `updated_at` column in merchants table

ALTER TABLE merchants
    ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER merchants_set_updated_at
    BEFORE UPDATE ON merchants
    FOR EACH ROW
    EXECUTE FUNCTION set_updated_at();
