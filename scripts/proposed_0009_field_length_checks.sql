-- #1131 — proposed second line of defence for signup field lengths.
-- `migrations/` is protected for contributors; maintainers should copy this
-- into `migrations/0009_field_length_checks.sql` (or the next free number)
-- and run it against production after reviewing.
--
-- Application-level checks (name ≤ 100, email shape / local ≤ 64 / domain ≤ 255,
-- phone normalized to E.164) already reject oversized input before insert.
-- These CHECKs catch anything that bypasses the API.

ALTER TABLE users
  ADD CONSTRAINT users_email_len CHECK (char_length(email) <= 254),
  ADD CONSTRAINT users_name_len CHECK (char_length(name) <= 100),
  ADD CONSTRAINT users_phone_len CHECK (phone_number IS NULL OR char_length(phone_number) <= 20);

ALTER TABLE otp_challenges
  ADD CONSTRAINT otp_pending_email_len CHECK (pending_email IS NULL OR char_length(pending_email) <= 254),
  ADD CONSTRAINT otp_pending_name_len CHECK (pending_name IS NULL OR char_length(pending_name) <= 100),
  ADD CONSTRAINT otp_phone_len CHECK (char_length(phone_number) <= 20);

ALTER TABLE merchants
  ADD CONSTRAINT merchants_name_len CHECK (char_length(name) <= 100);
