-- Migration: Database Scaling Architecture - Shard Registry
-- 
-- Creates the infrastructure for:
-- - Logical sharding by corridor
-- - Shard status management (active, draining, offline)
-- - Hot shard addition/removal

-- ─────────────────────────────────────────────────────────────────────────────
-- Shard Registry Table
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS shard_registry (
    shard_id INT PRIMARY KEY,
    
    -- Routing key: corridor_id + optional week_id
    corridor_id TEXT NOT NULL,
    week_id INT,
    
    -- Connection strings
    primary_dsn TEXT NOT NULL,
    replica_dsns TEXT[] DEFAULT ARRAY[]::TEXT[],
    
    -- Status: active (accepts writes), draining (read-only), offline
    status TEXT NOT NULL DEFAULT 'active' CHECK (
        status IN ('active', 'draining', 'offline')
    ),
    
    -- Connection tuning
    max_connections INT DEFAULT 8,
    min_connections INT DEFAULT 4,
    
    -- Load balancing weight
    weight INT DEFAULT 1,
    
    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for shard lookups
CREATE INDEX IF NOT EXISTS idx_shard_registry_corridor_week 
ON shard_registry(corridor_id, week_id);

CREATE INDEX IF NOT EXISTS idx_shard_registry_status 
ON shard_registry(status);

-- Comments for documentation
COMMENT ON TABLE shard_registry IS 
'Maintains shard configuration for logical sharding across corridors. Supports hot-reload and graceful shard migration via status transitions (active → draining → offline).';

COMMENT ON COLUMN shard_registry.status IS 
'Shard lifecycle: active (read+write), draining (read-only for migration), offline (no traffic)';

-- ─────────────────────────────────────────────────────────────────────────────
-- Trigger for updated_at
-- ─────────────────────────────────────────────────────────────────────────────

CREATE OR REPLACE FUNCTION update_shard_registry_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trigger_shard_registry_updated_at
    BEFORE UPDATE ON shard_registry
    FOR EACH ROW
    EXECUTE FUNCTION update_shard_registry_updated_at();

-- ─────────────────────────────────────────────────────────────────────────────
-- Initial Data (example for 3-corridor deployment)
-- ─────────────────────────────────────────────────────────────────────────────

-- INSERT INTO shard_registry (
--     shard_id, corridor_id, week_id, 
--     primary_dsn, replica_dsns,
--     status, max_connections, weight
-- ) VALUES
--     (0, 'NG', 202601, 'postgres://ng-w1-primary:5432/aframp', 
--      ARRAY['postgres://ng-w1-replica1:5432/aframp', 'postgres://ng-w1-replica2:5432/aframp'],
--      'active', 16, 1),
--     (1, 'GH', 202601, 'postgres://gh-w1-primary:5432/aframp',
--      ARRAY['postgres://gh-w1-replica1:5432/aframp'],
--      'active', 16, 1),
--     (2, 'KE', 202601, 'postgres://ke-w1-primary:5432/aframp',
--      ARRAY['postgres://ke-w1-replica1:5432/aframp', 'postgres://ke-w1-replica2:5432/aframp'],
--      'active', 16, 1)
-- ON CONFLICT (shard_id) DO NOTHING;

-- ─────────────────────────────────────────────────────────────────────────────
-- Rollback
-- ─────────────────────────────────────────────────────────────────────────────
-- DROP TRIGGER IF EXISTS trigger_shard_registry_updated_at ON shard_registry;
-- DROP FUNCTION IF EXISTS update_shard_registry_updated_at();
-- DROP INDEX IF EXISTS idx_shard_registry_status;
-- DROP INDEX IF EXISTS idx_shard_registry_corridor_week;
-- DROP TABLE IF EXISTS shard_registry;
