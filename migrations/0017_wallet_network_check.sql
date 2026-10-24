-- Restrict wallet.network to the allowlist the API can actually serve.
-- Requests are also validated in the handler; this constraint guards against
-- direct DB writes and future code paths that bypass the API layer.
ALTER TABLE wallets
    ADD CONSTRAINT wallets_network_check CHECK (network IN ('stellar', 'testnet'));
