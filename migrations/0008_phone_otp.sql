ALTER TABLE users ADD COLUMN phone_number TEXT UNIQUE;
ALTER TABLE users ADD COLUMN phone_verified BOOLEAN NOT NULL DEFAULT false;

CREATE TABLE otp_challenges (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  purpose TEXT NOT NULL CHECK (purpose IN ('signup', 'login')),
  user_id UUID REFERENCES users(id),
  pending_email TEXT,
  pending_password_hash TEXT,
  pending_name TEXT,
  phone_number TEXT NOT NULL,
  code_hash TEXT NOT NULL,
  attempts INT NOT NULL DEFAULT 0,
  max_attempts INT NOT NULL DEFAULT 5,
  expires_at TIMESTAMPTZ NOT NULL,
  consumed_at TIMESTAMPTZ,
  last_sent_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK (
    (purpose = 'login'  AND user_id IS NOT NULL AND pending_email IS NULL
                        AND pending_password_hash IS NULL AND pending_name IS NULL)
    OR
    (purpose = 'signup' AND user_id IS NULL AND pending_email IS NOT NULL
                        AND pending_password_hash IS NOT NULL AND pending_name IS NOT NULL)
  )
);

CREATE INDEX otp_challenges_phone_purpose_idx ON otp_challenges (phone_number, purpose, created_at DESC);
CREATE INDEX otp_challenges_user_idx ON otp_challenges (user_id) WHERE user_id IS NOT NULL;
