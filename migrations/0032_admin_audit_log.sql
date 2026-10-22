-- #1267: durable record of every authenticated admin request, independent of
-- whether a log aggregator is configured. Append-only from the application.
CREATE TABLE IF NOT EXISTS admin_audit_log (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Users are soft-deleted, so the reference never dangles; RESTRICT keeps a
    -- hard delete from silently erasing who did what.
    admin_user_id   UUID        NOT NULL REFERENCES users(id),
    action          TEXT        NOT NULL,
    resource_type   TEXT,
    resource_id     UUID,
    ip_address      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS admin_audit_log_admin_user_id_idx
    ON admin_audit_log (admin_user_id, created_at DESC);

CREATE INDEX IF NOT EXISTS admin_audit_log_resource_idx
    ON admin_audit_log (resource_type, resource_id)
    WHERE resource_type IS NOT NULL;
