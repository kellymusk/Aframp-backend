-- Migration 0009: add multi-level admin role to users
-- Introduces admin_role for field-level access control on admin endpoints.
-- Roles: 'super_admin', 'support', 'finance'. NULL for non-admin users.

ALTER TABLE users ADD COLUMN admin_role TEXT;

-- Backfill: existing admins default to the highest-privilege role so that
-- current behavior is preserved until roles are explicitly assigned.
UPDATE users
SET admin_role = 'super_admin'
WHERE is_admin = TRUE AND admin_role IS NULL;

-- Enforce the allowed role values at the database level.
ALTER TABLE users
    ADD CONSTRAINT users_admin_role_check
    CHECK (admin_role IS NULL OR admin_role IN ('super_admin', 'support', 'finance'));

-- Support role-based lookups on admin endpoints.
CREATE INDEX idx_users_admin_role ON users (admin_role) WHERE admin_role IS NOT NULL;
