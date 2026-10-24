-- POST /auth/change-password stamps this column. Any session whose original
-- login (`orig_iat`, carried unchanged through refreshes) predates it is
-- rejected, so changing a password signs every other device out.
ALTER TABLE users ADD COLUMN password_changed_at TIMESTAMPTZ;
