-- Performance: indexes for admin list queries in src/services/admin.rs
-- All admin list queries ORDER BY created_at DESC; without these indexes
-- Postgres does a sequential scan plus an in-memory sort per request.

-- users: admin list ordered by created_at DESC
CREATE INDEX IF NOT EXISTS idx_users_created_at
    ON users (created_at DESC);

-- merchants: admin list ordered by created_at DESC
CREATE INDEX IF NOT EXISTS idx_merchants_created_at
    ON merchants (created_at DESC);

-- wallets: admin list ordered by created_at DESC
CREATE INDEX IF NOT EXISTS idx_wallets_created_at
    ON wallets (created_at DESC);

-- payments: admin list ordered by created_at DESC
CREATE INDEX IF NOT EXISTS idx_payments_created_at
    ON payments (created_at DESC);

-- payments: admin queries filter by merchant_id and ORDER BY created_at DESC
CREATE INDEX IF NOT EXISTS idx_payments_merchant_id_created_at
    ON payments (merchant_id, created_at DESC);

-- withdrawals: admin list ordered by created_at DESC
CREATE INDEX IF NOT EXISTS idx_withdrawals_created_at
    ON withdrawals (created_at DESC);

-- withdrawals: admin queries filter by merchant_id and ORDER BY created_at DESC
CREATE INDEX IF NOT EXISTS idx_withdrawals_merchant_id_created_at
    ON withdrawals (merchant_id, created_at DESC);

-- payment_requests: admin list ordered by created_at DESC
CREATE INDEX IF NOT EXISTS idx_payment_requests_created_at
    ON payment_requests (created_at DESC);

-- payment_requests: admin queries filter by merchant_id and ORDER BY created_at DESC
CREATE INDEX IF NOT EXISTS idx_payment_requests_merchant_id_created_at
    ON payment_requests (merchant_id, created_at DESC);
