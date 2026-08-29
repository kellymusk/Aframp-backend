CREATE TABLE payment_requests (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  merchant_id UUID NOT NULL REFERENCES merchants(id),
  wallet_id UUID NOT NULL REFERENCES wallets(id),
  amount_stroops BIGINT NOT NULL CHECK (amount_stroops > 0),
  asset TEXT NOT NULL DEFAULT 'cNGN',
  memo TEXT NOT NULL UNIQUE,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'paid', 'expired')),
  payment_id UUID REFERENCES payments(id),
  expires_at TIMESTAMPTZ NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_payment_requests_wallet_memo_pending
  ON payment_requests (wallet_id, memo)
  WHERE status = 'pending';
