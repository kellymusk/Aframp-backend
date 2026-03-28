-- Mint Authorization Audit Trail (cNGN minting)
-- This table is append-only and chain-verified for tamper-evidence.

CREATE TYPE mint_action_type AS ENUM (
    'mint_requested',
    'mint_approved',
    'mint_submitted',
    'mint_completed',
    'mint_failed'
);

CREATE TABLE mint_authorization_logs (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    actor_id TEXT NOT NULL,
    public_key TEXT NOT NULL,
    action_type mint_action_type NOT NULL,
    request_payload JSONB NOT NULL,
    previous_hash TEXT NOT NULL,
    current_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id)
);

CREATE INDEX idx_mint_authorization_logs_created_at ON mint_authorization_logs (created_at);
CREATE INDEX idx_mint_authorization_logs_actor_id ON mint_authorization_logs (actor_id, created_at);
CREATE INDEX idx_mint_authorization_logs_action_type ON mint_authorization_logs (action_type, created_at);

CREATE OR REPLACE FUNCTION mint_authorization_log_immutable()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'mint_authorization_logs is append-only: % on mint_authorization_logs is forbidden', TG_OP;
END;
$$;

CREATE TRIGGER trg_mint_authorization_log_no_update
    BEFORE UPDATE ON mint_authorization_logs
    FOR EACH ROW EXECUTE FUNCTION mint_authorization_log_immutable();

CREATE TRIGGER trg_mint_authorization_log_no_delete
    BEFORE DELETE ON mint_authorization_logs
    FOR EACH ROW EXECUTE FUNCTION mint_authorization_log_immutable();
