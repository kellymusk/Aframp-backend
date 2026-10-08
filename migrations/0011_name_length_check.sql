-- Enforce the 100-character name limit at the database level.
--
-- `validate_name` in `src/validation.rs` already rejects names longer than
-- 100 characters, but the `users.name` and `merchants.name` columns are plain
-- `TEXT` with no constraint. If validation is ever bypassed (direct DB insert,
-- admin endpoint, a future endpoint that forgets to validate), arbitrarily
-- long names could be stored. This migration makes the database the source of
-- truth for the limit while the application-level check remains the
-- first-line, user-friendly validation.

ALTER TABLE users
    ADD CONSTRAINT users_name_length_check
    CHECK (char_length(name) <= 100);

ALTER TABLE merchants
    ADD CONSTRAINT merchants_name_length_check
    CHECK (char_length(name) <= 100);
