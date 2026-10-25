-- #1298: admin roles for field-level access on /admin/*. NULL for non-admins.
--   super_admin: everything; finance: money (overview, withdrawals);
--   support: people (users, merchants and their emails).
ALTER TABLE users ADD COLUMN IF NOT EXISTS admin_role TEXT;

-- Existing admins keep full access until roles are assigned explicitly.
UPDATE users SET admin_role = 'super_admin' WHERE is_admin AND admin_role IS NULL;

ALTER TABLE users
    ADD CONSTRAINT users_admin_role_check
    CHECK (admin_role IS NULL OR admin_role IN ('super_admin', 'support', 'finance'));
