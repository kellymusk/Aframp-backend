-- Bank accounts registered/verified per merchant.
-- Withdrawals must reference a bank_account_id owned by the authenticated merchant
-- instead of accepting free-form bank_code/account_number inputs.

CREATE TABLE IF NOT EXISTS bank_accounts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_id UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    bank_code TEXT NOT NULL,
    account_number TEXT NOT NULL,
    account_name TEXT,
    is_verified BOOLEAN NOT NULL DEFAULT FALSE,
    verified_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT bank_accounts_merchant_account_unique UNIQUE (merchant_id, bank_code, account_number)
);

CREATE INDEX IF NOT EXISTS idx_bank_accounts_merchant_id
    ON bank_accounts (merchant_id);
