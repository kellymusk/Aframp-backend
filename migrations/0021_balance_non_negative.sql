-- Issue #946: Add CHECK constraints to prevent negative balances
ALTER TABLE balances
  ADD CONSTRAINT check_available_non_negative CHECK (available >= 0),
  ADD CONSTRAINT check_pending_non_negative CHECK (pending >= 0);
