-- Account deletion (DELETE /me) is a soft delete: personal data is
-- anonymized in place and the account marked deleted, so payments,
-- withdrawals and balances keep a valid owner for the financial audit trail.
ALTER TABLE users ADD COLUMN deleted_at TIMESTAMPTZ;
