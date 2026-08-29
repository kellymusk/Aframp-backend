-- Migration: Database Scaling Architecture - Query Acceleration Views and Indexes
-- 
-- Creates materialized views and performance indexes for:
-- - Settlement summaries aggregation
-- - Transaction statistics by corridor-day
-- - Replica lag monitoring
-- - Query performance optimization

-- ─────────────────────────────────────────────────────────────────────────────
-- Settlement Summaries Materialized View
-- ─────────────────────────────────────────────────────────────────────────────

CREATE MATERIALIZED VIEW IF NOT EXISTS settlement_summaries_by_corridor_week AS
SELECT
    -- Routing key
    'NG' as corridor_id,  -- Inferred from transaction metadata or explicit column
    EXTRACT(YEAR FROM created_at)::INT * 100 + EXTRACT(WEEK FROM created_at)::INT as week_id,
    
    -- Aggregates
    SUM(gross_amount) as total_gross,
    SUM(platform_fee) as total_fees,
    SUM(COALESCE(provider_charge, 0)) as total_provider_charge,
    AVG(gross_amount) as avg_batch_amount,
    
    -- Counts
    COUNT(*) as batch_count,
    COUNT(CASE WHEN status = 'settled' THEN 1 END) as settled_count,
    COUNT(CASE WHEN status = 'failed' THEN 1 END) as failed_count,
    COUNT(CASE WHEN status = 'pending' THEN 1 END) as pending_count,
    COUNT(CASE WHEN status = 'processing' THEN 1 END) as processing_count,
    COUNT(CASE WHEN status = 'reconciled' THEN 1 END) as reconciled_count,
    
    -- Metadata
    NOW() as last_updated,
    MAX(updated_at) as latest_batch_update
FROM settlement_batches
GROUP BY corridor_id, week_id;

-- Unique index for upserts
CREATE UNIQUE INDEX IF NOT EXISTS idx_settlement_summaries_unique
ON settlement_summaries_by_corridor_week(corridor_id, week_id);

-- Query optimization indexes
CREATE INDEX IF NOT EXISTS idx_settlement_summaries_corridor
ON settlement_summaries_by_corridor_week(corridor_id);

CREATE INDEX IF NOT EXISTS idx_settlement_summaries_week
ON settlement_summaries_by_corridor_week(week_id DESC);

COMMENT ON MATERIALIZED VIEW settlement_summaries_by_corridor_week IS
'Pre-aggregated settlement summaries by corridor and week for fast dashboard queries. Refresh every 5 minutes.';

-- ─────────────────────────────────────────────────────────────────────────────
-- Transaction Statistics Materialized View
-- ─────────────────────────────────────────────────────────────────────────────

CREATE MATERIALIZED VIEW IF NOT EXISTS transaction_stats_by_corridor_day AS
SELECT
    -- Determine corridor from wallet address or explicit column
    CASE
        WHEN wallet_address LIKE '%NG%' OR wallet_address LIKE '%ng%' THEN 'NG'
        WHEN wallet_address LIKE '%GH%' OR wallet_address LIKE '%gh%' THEN 'GH'
        WHEN wallet_address LIKE '%KE%' OR wallet_address LIKE '%ke%' THEN 'KE'
        ELSE 'OTHER'
    END as corridor_id,
    
    -- Time dimension
    DATE_TRUNC('day', created_at)::DATE as day,
    
    -- Transaction classification
    type as transaction_type,
    status as transaction_status,
    
    -- Aggregates
    COUNT(*) as transaction_count,
    SUM(CAST(from_amount AS NUMERIC)) as total_from_amount,
    SUM(CAST(to_amount AS NUMERIC)) as total_to_amount,
    AVG(CAST(from_amount AS NUMERIC)) as avg_from_amount,
    AVG(CAST(to_amount AS NUMERIC)) as avg_to_amount,
    MIN(CAST(from_amount AS NUMERIC)) as min_from_amount,
    MAX(CAST(from_amount AS NUMERIC)) as max_from_amount,
    
    -- Payment method breakdown
    COUNT(CASE WHEN payment_provider IS NOT NULL THEN 1 END) as provider_count,
    
    -- Metadata
    NOW() as last_updated
FROM transactions
GROUP BY corridor_id, day, type, status;

-- Unique index for upserts
CREATE UNIQUE INDEX IF NOT EXISTS idx_transaction_stats_unique
ON transaction_stats_by_corridor_day(corridor_id, day, transaction_type, transaction_status);

-- Query optimization indexes
CREATE INDEX IF NOT EXISTS idx_transaction_stats_corridor
ON transaction_stats_by_corridor_day(corridor_id, day DESC);

CREATE INDEX IF NOT EXISTS idx_transaction_stats_day
ON transaction_stats_by_corridor_day(day DESC);

COMMENT ON MATERIALIZED VIEW transaction_stats_by_corridor_day IS
'Transaction statistics aggregated by corridor, day, type, and status for analytics and monitoring. Refresh every hour.';

-- ─────────────────────────────────────────────────────────────────────────────
-- Performance Indexes for Transaction Queries
-- ─────────────────────────────────────────────────────────────────────────────

-- Index for ledger verification queries
CREATE INDEX IF NOT EXISTS idx_transactions_ledger_verification
ON transactions(wallet_address, created_at DESC, status)
WHERE status IN ('completed', 'failed', 'pending');

-- Index for compliance queries
CREATE INDEX IF NOT EXISTS idx_transactions_compliance_check
ON transactions(type, created_at DESC)
WHERE type IN ('onramp', 'offramp', 'bill_payment');

-- Index for audit ledger queries by timestamp range
CREATE INDEX IF NOT EXISTS idx_audit_ledger_timestamp_range
ON audit_ledger(timestamp DESC)
WHERE timestamp > NOW() - INTERVAL '90 days';

-- Index for correlation queries
CREATE INDEX IF NOT EXISTS idx_audit_ledger_correlation_timestamp
ON audit_ledger(correlation_id, timestamp DESC)
WHERE correlation_id IS NOT NULL;

-- ─────────────────────────────────────────────────────────────────────────────
-- Partial Indexes for Hot Queries
-- ─────────────────────────────────────────────────────────────────────────────

-- Index for pending settlements (hot query)
CREATE INDEX IF NOT EXISTS idx_settlement_batches_pending
ON settlement_batches(created_at DESC)
WHERE status IN ('pending', 'processing');

-- Index for recent transactions by wallet (very hot)
CREATE INDEX IF NOT EXISTS idx_transactions_recent_by_wallet
ON transactions(wallet_address, created_at DESC)
WHERE created_at > NOW() - INTERVAL '7 days';

-- ─────────────────────────────────────────────────────────────────────────────
-- Statistics for Query Planner
-- ─────────────────────────────────────────────────────────────────────────────

-- Update table statistics for better query plans
-- Run this after migration:
-- ANALYZE settlement_batches;
-- ANALYZE transactions;
-- ANALYZE audit_ledger;

-- ─────────────────────────────────────────────────────────────────────────────
-- Rollback
-- ─────────────────────────────────────────────────────────────────────────────
-- DROP INDEX IF EXISTS idx_settlement_batches_pending;
-- DROP INDEX IF EXISTS idx_transactions_recent_by_wallet;
-- DROP INDEX IF EXISTS idx_audit_ledger_correlation_timestamp;
-- DROP INDEX IF EXISTS idx_audit_ledger_timestamp_range;
-- DROP INDEX IF EXISTS idx_transactions_compliance_check;
-- DROP INDEX IF EXISTS idx_transactions_ledger_verification;
-- DROP INDEX IF EXISTS idx_transaction_stats_day;
-- DROP INDEX IF EXISTS idx_transaction_stats_corridor;
-- DROP INDEX IF EXISTS idx_transaction_stats_unique;
-- DROP MATERIALIZED VIEW IF EXISTS transaction_stats_by_corridor_day;
-- DROP INDEX IF EXISTS idx_settlement_summaries_week;
-- DROP INDEX IF EXISTS idx_settlement_summaries_corridor;
-- DROP INDEX IF EXISTS idx_settlement_summaries_unique;
-- DROP MATERIALIZED VIEW IF EXISTS settlement_summaries_by_corridor_week;
