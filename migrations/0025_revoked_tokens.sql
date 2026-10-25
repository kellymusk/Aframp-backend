-- JWT IDs revoked by POST /logout. Rows are only needed until the token
-- would have expired anyway; expired rows are pruned on each revocation.
CREATE TABLE revoked_tokens (
  jti UUID PRIMARY KEY,
  expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX revoked_tokens_expires_at_idx ON revoked_tokens (expires_at);
