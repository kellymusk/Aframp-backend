-- Allow an OTP challenge that verifies a new phone number for an existing
-- account (PATCH /me). Like 'login' it belongs to a user and carries no
-- pending signup fields; `phone_number` is the new number being verified.

ALTER TABLE otp_challenges DROP CONSTRAINT otp_challenges_purpose_check;
ALTER TABLE otp_challenges ADD CONSTRAINT otp_challenges_purpose_check
  CHECK (purpose IN ('signup', 'login', 'phone_change'));

ALTER TABLE otp_challenges DROP CONSTRAINT otp_challenges_check;
ALTER TABLE otp_challenges ADD CONSTRAINT otp_challenges_check CHECK (
  (purpose IN ('login', 'phone_change') AND user_id IS NOT NULL AND pending_email IS NULL
                                        AND pending_password_hash IS NULL AND pending_name IS NULL)
  OR
  (purpose = 'signup' AND user_id IS NULL AND pending_email IS NOT NULL
                      AND pending_password_hash IS NOT NULL AND pending_name IS NOT NULL)
);
