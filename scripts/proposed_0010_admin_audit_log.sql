-- #1267 — proposed migration: admin_audit_log table.
-- `migrations/` is protected for contributors; maintainers should copy this
-- into `migrations/0010_admin_audit_log.sql` (or the next free number) and
-- run it against production after reviewing.
--
-- Every call to an admin endpoint (`/admin/*`) inserts a row here in addition
-- to the existing tracing::info! log.  This makes admin actions durable even
-- if the log aggregator is not configured or logs are rotated.
--
-- The row captures:
--   id              — surrogate key (UUID v4)
--   admin_user_id   — the user.id from the JWT; NOT NULL (every admin action
--                     must be tied to an authenticated user)
--   action          — the HTTP method + path, e.g. "GET /admin/users".
--                     Trim to 255 chars to bound storage.
--   resource_type   — high-level resource name derived from the path, e.g.
--                     "users", "wallets", "transactions".  NULL if unknown.
--   resource_id     — the UUID from the path parameter, e.g. the user id for
--                     GET /admin/users/:id.  NULL for list/overview endpoints.
--   ip_address      — client IP, captured from the X-Forwarded-For header
--                     (or the direct peer address when behind no proxy).
--                     Stored as TEXT rather than inet for portability; NULL
--                     when the IP is unavailable.
--   created_at      — wall-clock time of the admin action (UTC).
--
-- This table is append-only; rows are never updated or deleted by the
-- application.  Periodic archival / retention enforcement is a separate
-- concern.

CREATE TABLE IF NOT EXISTS admin_audit_log (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    admin_user_id   UUID        NOT NULL REFERENCES users(id) ON DELETE SET NULL,
    action          TEXT        NOT NULL,
    resource_type   TEXT,
    resource_id     UUID,
    ip_address      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Allow efficient queries like "show me all actions by admin X" and
-- "show me all recent actions on resource Y".
CREATE INDEX IF NOT EXISTS admin_audit_log_admin_user_id_idx
    ON admin_audit_log (admin_user_id);

CREATE INDEX IF NOT EXISTS admin_audit_log_resource_type_resource_id_idx
    ON admin_audit_log (resource_type, resource_id)
    WHERE resource_type IS NOT NULL;

CREATE INDEX IF NOT EXISTS admin_audit_log_created_at_idx
    ON admin_audit_log (created_at DESC);
