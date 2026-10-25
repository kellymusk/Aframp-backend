-- #1292: optional merchant-defined description on a payment request (e.g.
-- "Table 5 order"), shown to the payer and carried as the SEP-7 `msg`.
ALTER TABLE payment_requests ADD COLUMN IF NOT EXISTS description TEXT;
