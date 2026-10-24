-- #1239 — PROPOSED migration (maintainers: copy into migrations/ when accepted;
-- contributors cannot modify migrations/ per CONTRIBUTING.md).
--
-- balances::apply_delta adds deltas with no floor check, so a caller passing a
-- negative available_delta can drive a balance below zero. A CHECK constraint
-- enforces the invariant for every writer, not just apply_delta.
--
-- Pre-flight: this fails if any row is already negative. Inspect first with
--   SELECT * FROM balances WHERE available < 0 OR pending < 0;

ALTER TABLE balances
    ADD CONSTRAINT balances_available_non_negative CHECK (available >= 0),
    ADD CONSTRAINT balances_pending_non_negative CHECK (pending >= 0);
