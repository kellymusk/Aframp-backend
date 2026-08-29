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

CREATE TABLE api_keys (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  merchant_id UUID NOT NULL REFERENCES merchants(id),
  key_prefix TEXT NOT NULL,
  secret_hash TEXT NOT NULL,
  environment TEXT NOT NULL CHECK (environment IN ('test', 'live')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  revoked_at TIMESTAMPTZ
);

CREATE TABLE webhook_events (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  merchant_id UUID NOT NULL REFERENCES merchants(id),
  provider TEXT NOT NULL,
  external_id TEXT NOT NULL,
  payload JSONB NOT NULL,
  received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (provider, external_id)
);
