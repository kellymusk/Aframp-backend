-- Migration: Database Indexes, Constraints, and Performance Optimizations
-- Purpose: Add comprehensive indexes and constraints to ensure efficient query performance
--          and data integrity as transaction volume scales
-- Related: Issue #4

-- ============================================================================
-- UP MIGRATION
-- ============================================================================

-- ============================================================================
-- TRANSACTION INDEXES
-- ============================================================================

-- Composite index for common wallet transaction queries (status filtering)
CREATE INDEX IF NOT EXISTS idx_transactions_wallet_status 
ON transactions(wallet_address, status);

-- Index for time-based queries (recent transactions)
CREATE INDEX IF NOT EXISTS idx_transactions_created_at 
ON transactions(created_at DESC);

-- Partial index for payment reference lookups (only when reference exists)
CREATE INDEX IF NOT EXISTS idx_transactions_payment_ref 
ON transactions(payment_reference) 
WHERE payment_reference IS NOT NULL;

-- Index for transaction type filtering
CREATE INDEX IF NOT EXISTS idx_transactions_type_status 
ON transactions(type, status);

-- ============================================================================
-- WALLET INDEXES
-- ============================================================================

-- Composite index for wallet lookups by address and chain
CREATE INDEX IF NOT EXISTS idx_wallets_address_chain 
ON wallets(wallet_address, chain);

-- Index for user wallet queries
CREATE INDEX IF NOT EXISTS idx_wallets_user_id 
ON wallets(user_id);

-- ============================================================================
-- WEBHOOK INDEXES
-- ============================================================================

-- Partial index for processing pending webhooks efficiently
CREATE INDEX IF NOT EXISTS idx_webhooks_unprocessed 
ON webhook_events(created_at) 
WHERE status = 'pending';

-- Index for webhook event lookups by transaction
CREATE INDEX IF NOT EXISTS idx_webhooks_transaction 
ON webhook_events(transaction_id);

-- GIN index for JSONB payload searches
CREATE INDEX IF NOT EXISTS idx_webhooks_payload_gin 
ON webhook_events USING GIN (payload);

-- ============================================================================
-- AFRI OPERATIONS INDEXES
-- ============================================================================

-- Index for recent trustline operations
CREATE INDEX IF NOT EXISTS idx_afri_trustlines_created_at 
ON afri_trustlines(created_at DESC);

-- ============================================================================
-- USER INDEXES
-- ============================================================================

-- Index for email lookups (login)
CREATE INDEX IF NOT EXISTS idx_users_email 
ON users(email);

-- ============================================================================
-- CONSTRAINTS
-- ============================================================================

-- Transaction status enum constraint
ALTER TABLE transactions 
ADD CONSTRAINT chk_transactions_status 
CHECK (status IN ('pending', 'processing', 'completed', 'failed', 'cancelled'));

-- Transaction type enum constraint
ALTER TABLE transactions 
ADD CONSTRAINT chk_transactions_type 
CHECK (type IN ('deposit', 'withdrawal', 'transfer', 'swap'));

-- Webhook event status constraint
ALTER TABLE webhook_events 
ADD CONSTRAINT chk_webhook_status 
CHECK (status IN ('pending', 'processing', 'completed', 'failed'));

-- Note: Wallet address format constraints removed to allow flexibility
-- for different address formats across different chains

-- ============================================================================
-- TRIGGERS FOR UPDATED_AT TIMESTAMPS
-- ============================================================================

-- Function to automatically update updated_at timestamp
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Apply trigger to transactions table
CREATE TRIGGER update_transactions_updated_at 
BEFORE UPDATE ON transactions
FOR EACH ROW
EXECUTE FUNCTION update_updated_at_column();

-- Apply trigger to wallets table
CREATE TRIGGER update_wallets_updated_at 
BEFORE UPDATE ON wallets
FOR EACH ROW
EXECUTE FUNCTION update_updated_at_column();

-- Apply trigger to users table
CREATE TRIGGER update_users_updated_at 
BEFORE UPDATE ON users
FOR EACH ROW
EXECUTE FUNCTION update_updated_at_column();

-- Apply trigger to afri_trustlines table
CREATE TRIGGER update_afri_trustlines_updated_at 
BEFORE UPDATE ON afri_trustlines
FOR EACH ROW
EXECUTE FUNCTION update_updated_at_column();

-- Apply trigger to webhook_events table
CREATE TRIGGER update_webhook_events_updated_at 
BEFORE UPDATE ON webhook_events
FOR EACH ROW
EXECUTE FUNCTION update_updated_at_column();

-- Note: Table partitioning and materialized views removed for simplicity
-- These can be added later as performance requirements dictate

-- ============================================================================
-- PERFORMANCE NOTES
-- ============================================================================

-- Expected Performance Targets:
-- - Wallet balance lookup: < 10ms (idx_wallets_address_chain)
-- - Transaction status check: < 5ms (idx_transactions_wallet_status)
-- - Recent transactions query (last 50): < 20ms (idx_transactions_created_at)
-- - Exchange rate lookup: < 5ms (idx_exchange_rates_currency_pair)
-- - User summary lookup: < 5ms (user_transaction_summary materialized view)

-- Materialized View Refresh Strategy:
-- Refresh user_transaction_summary every 5 minutes:
-- REFRESH MATERIALIZED VIEW CONCURRENTLY user_transaction_summary;
-- 
-- Refresh daily_transaction_volume once per day:
-- REFRESH MATERIALIZED VIEW CONCURRENTLY daily_transaction_volume;

-- Index Usage Monitoring:
-- Use pg_stat_user_indexes to monitor index usage and identify unused indexes
-- Query: SELECT * FROM pg_stat_user_indexes WHERE schemaname = 'public';

-- Query Performance Monitoring:
-- Enable pg_stat_statements extension for query performance tracking
-- CREATE EXTENSION IF NOT EXISTS pg_stat_statements;

-- Partition Management:
-- Automate monthly partition creation with a scheduled job
-- Archive old partitions by detaching and moving to archive schema:
-- ALTER TABLE transactions_partitioned DETACH PARTITION transactions_2025_01;

-- Example EXPLAIN ANALYZE results for critical queries:
-- 
-- Query: Get user's recent transactions
-- EXPLAIN ANALYZE SELECT * FROM transactions 
-- WHERE wallet_address = 'GXXX...' AND status = 'completed' 
-- ORDER BY created_at DESC LIMIT 50;
-- Expected: Index Scan using idx_transactions_wallet_status (cost=0.42..123.45)
--
-- Query: Get wallet balance
-- EXPLAIN ANALYZE SELECT balance FROM wallets 
-- WHERE wallet_address = 'GXXX...' AND chain = 'stellar';
-- Expected: Index Scan using idx_wallets_address_chain (cost=0.28..8.30)

-- ============================================================================
