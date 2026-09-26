-- Last line of defence against a balance going negative: any write that
-- would take `available` below zero now fails instead of being stored.
ALTER TABLE balances
  ADD CONSTRAINT balances_available_non_negative CHECK (available >= 0);
