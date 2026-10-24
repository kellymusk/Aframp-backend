-- Initial schema for the application.
--
-- NOTE: The `api_keys` table below is planned infrastructure. It is created
-- here so the schema is ready, but there are currently no API endpoints to
-- create, list, or revoke API keys, and the `ApiKey` model in
-- `src/models/api_key.rs` is not yet wired to any handlers. API key
-- authentication (including multi-environment keys) is a planned feature;
-- see PRD.md. Do not assume this table is in active use.

CREATE TABLE IF NOT EXISTS api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE users (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  email TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  name TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE merchants (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL REFERENCES users(id),
  name TEXT NOT NULL,
  kyc_status TEXT NOT NULL DEFAULT 'unverified' CHECK (kyc_status IN ('unverified', 'pending', 'verified', 'rejected')),
  bvn TEXT,
  cac_number TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE wallets (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  merchant_id UUID NOT NULL REFERENCES merchants(id),
  address TEXT NOT NULL UNIQUE,
  network TEXT NOT NULL DEFAULT 'stellar',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE payments (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  merchant_id UUID NOT NULL REFERENCES merchants(id),
  wallet_id UUID NOT NULL REFERENCES wallets(id),
  wallet_address TEXT NOT NULL,
  tx_hash TEXT NOT NULL UNIQUE,
  amount_stroops BIGINT NOT NULL,
  asset TEXT NOT NULL DEFAULT 'cNGN',
  network TEXT NOT NULL DEFAULT 'stellar',
  status TEXT NOT NULL CHECK (status IN ('detected', 'verified', 'confirmed', 'failed')),
  confirmations INT NOT NULL DEFAULT 0,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE balances (
  merchant_id UUID NOT NULL REFERENCES merchants(id),
  asset TEXT NOT NULL,
  available BIGINT NOT NULL DEFAULT 0,
  pending BIGINT NOT NULL DEFAULT 0,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (merchant_id, asset)
);

CREATE TABLE withdrawals (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  merchant_id UUID NOT NULL REFERENCES merchants(id),
  amount_stroops BIGINT NOT NULL,
  asset TEXT NOT NULL DEFAULT 'cNGN',
  status TEXT NOT NULL CHECK (status IN ('pending', 'processing', 'completed', 'failed')),
  provider TEXT,
  provider_reference TEXT UNIQUE,
  bank_code TEXT,
  account_number TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- api_keys: reserved for the planned API key authentication system (see PRD.md).
-- Not yet referenced by any Rust code; kept in the schema so the auth feature can
-- be implemented without a breaking migration. key_prefix stores the public,
-- non-secret portion of the key for lookup, secret_hash stores a hash of the
-- secret portion, environment distinguishes test vs live keys, and revoked_at
-- marks keys that have been revoked (NULL means active).
CREATE TABLE api_keys (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  merchant_id UUID NOT NULL REFERENCES merchants(id),
  key_prefix TEXT NOT NULL,
  secret_hash TEXT NOT NULL,
  environment TEXT NOT NULL CHECK (environment IN ('test', 'live')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  revoked_at TIMESTAMPTZ
);

-- Audit trail for inbound provider webhooks (Termii, Paystack, ...).
-- merchant_id is nullable because some providers (e.g. Termii SMS delivery
-- receipts) are not merchant-specific; such events are still recorded so the
-- table reflects a real audit trail rather than an always-empty placeholder.
-- webhook_events: reserved for the planned inbound webhook handling system.
-- Not yet referenced by any Rust code; intended to persist raw provider webhook
-- payloads for idempotent processing and replay. The UNIQUE (provider,
-- external_id) constraint deduplicates repeated deliveries of the same event.
CREATE TABLE webhook_events (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  merchant_id UUID REFERENCES merchants(id),
  provider TEXT NOT NULL,
  event_type TEXT,
  external_id TEXT NOT NULL,
  payload JSONB NOT NULL,
  received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (provider, external_id)
);

-- Issue #1207: idempotency key support for POST /withdraw.
-- Stores the optional Idempotency-Key header supplied by the client so that
-- retried requests return the original withdrawal instead of creating a new one.
ALTER TABLE withdrawals ADD COLUMN idempotency_key TEXT;

-- Enforce uniqueness per merchant, ignoring rows without a key.
CREATE UNIQUE INDEX withdrawals_merchant_idempotency_key_idx
  ON withdrawals (merchant_id, idempotency_key)
  WHERE idempotency_key IS NOT NULL;
