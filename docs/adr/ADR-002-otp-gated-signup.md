# ADR-002: OTP-gated signup and login (phone-number 2FA)

**Status:** Accepted  
**Date:** 2024-11-01  
**Deciders:** Core team

---

## Context

Aframp merchants handle real money. An unverified or easily-faked identity at
signup is a direct fraud risk. Two common verification mechanisms were
considered:

**Option A — Email verification link**  
After collecting credentials, send a tokenised magic link to the supplied email
address. The account is created and activated when the user clicks the link.

**Option B — Phone OTP (SMS)**  
After collecting credentials, send a 6-digit one-time code to the supplied
Nigerian mobile number. The account is only ever materialised (written to the
`users` table) once the code is verified.

---

## Decision

**Option B (phone OTP) was chosen, for both signup and login (full 2FA).**

Rationale:

1. **Nigerian mobile penetration vs. email deliverability.**  
   The target market is Nigerian merchants. Mobile numbers are universal;
   deliverability of transactional email in Nigeria is notoriously unreliable
   across carriers and regions. An SMS that arrives in seconds is more useful
   than an email that might hit a spam folder.

2. **The OTP gates account creation, not just session issuance.**  
   A design that creates the `users` row at `/signup` and then demands OTP
   verification before issuing the session token is bypassable: an attacker who
   can call `/signup` still creates a database row (and can enumerate taken
   emails). With the chosen design, `/signup` only creates an `otp_challenges`
   row — the pending credentials (email, password hash, name) are stored there,
   and the `users` row is written only inside `services::otp::verify` after the
   code is confirmed. See the inline comment in that function for this
   reasoning.

3. **Full 2FA on login, not just signup.**  
   Every login for an account with a phone on file goes through the same
   challenge flow. `/login` verifies the password and then returns
   `{ challenge_id, expires_in_secs }` rather than a session — the session is
   issued only from `/verify-otp`. This means stolen-password attacks require
   access to the merchant's registered phone as well.

4. **Resend is a re-POST, not a separate endpoint.**  
   Re-submitting `/signup` or `/login` with the same credentials is the resend
   path. This keeps the surface area small and avoids a separate unauthenticated
   resend endpoint. Rate limiting (60 s between resends, 5 per hour per phone
   per purpose) prevents abuse.

---

## OTP code length and brute-force risk (issue #1422)

`services::otp::generate_code()` produces a **6-digit decimal** code, i.e. a
keyspace of `10^6 = 1,000,000` values. This length was chosen deliberately:

- **Usability.** 6 digits is the de-facto SMS OTP convention (RFC 4226 / HOTP
  and most banks use 6). It is short enough to read from a lock-screen
  notification and retype without errors, which matters for the target market.
- **SMS cost / length.** A 6-digit code keeps the SMS body within a single
  segment on Nigerian carriers, minimising delivery cost and failure.
- **Mitigations carry the security weight.** The keyspace alone is not the
  defence; the surrounding controls are.

### Current mitigations

| Control | Value | Where |
| --- | --- | --- |
| Max verification attempts per challenge | `MAX_ATTEMPTS = 5` | `src/services/otp.rs` |
| Challenge TTL | `CODE_TTL_SECS = 600` (10 minutes) | `src/services/otp.rs` |
| Resend cooldown | 60 s between resends, 5/hour per phone per purpose | `src/api/auth.rs` |
| Login rate limiting | per-IP, applied before a challenge is issued | `src/api/auth.rs` |

### Per-challenge brute-force probability

An attacker who knows the `challenge_id` (a public UUID returned by
`/signup` and `/login`) can submit at most `MAX_ATTEMPTS = 5` guesses before
the challenge is locked. The probability of guessing the correct code within
those 5 attempts is:

```
P(success) = MAX_ATTEMPTS / keyspace
           = 5 / 1,000,000
           = 0.000005
           = 0.0005%  (1 in 200,000 challenges)
```

This is documented and asserted by the `brute_force_probability_is_bounded`
test in `src/services/otp.rs`, which fails if `MAX_ATTEMPTS`, the code length,
or `CODE_TTL_SECS` drift from the values this analysis assumes.

### Acceptable risk threshold

We accept a per-challenge success probability of **≤ 0.001% (1 in 100,000)**.
The current `5 / 1,000,000 = 0.0005%` sits comfortably under that threshold.
The threshold is only meaningful in combination with the attempt cap and TTL:
without `MAX_ATTEMPTS`, the same keyspace would be trivially exhaustible.

If the attempt cap is ever raised, the code length must be raised in lockstep
so that `MAX_ATTEMPTS / 10^digits` stays below the threshold — the test above
enforces this relationship.

---

## Consequences

**Positive:**

- Phone number is verified before any account state is written, eliminating a
  class of partially-created ghost accounts.
- 2FA is mandatory for all phone-registered accounts — no opt-in required.
- The `phone_number` column carries a `UNIQUE` constraint, preventing duplicate
  account creation for the same number even under concurrent requests
  (the `otp_challenges.CHECK` constraint also separates signup and login
  challenge shapes at the schema level).

**Negative / open items:**

- Requires `TERMII_API_KEY` (or `OTP_PROVIDER=mock` for local dev) — the server
  refuses to start without a configured OTP provider.
- Admin accounts with a `phone_number` are now subject to the same challenge
  flow on login. The `/admin` dashboard's login form does not yet handle the
  two-step flow, so admin accounts should remain phone-less until the dashboard
  is updated. See the known gap in `README.md`.
- Pre-OTP accounts (no `phone_number`) continue to log in synchronously via
  the legacy path in `src/api/auth.rs` — this is intentional backward
  compatibility, not a security bypass, since those accounts predate the phone
  requirement.
- OTP delivery status (Termii webhook) is not yet correlated back to the
  `otp_challenges` row — a delivery failure is visible only in logs.
- Per-challenge brute-force risk is bounded but non-zero (see the section
  above). Future hardening could add per-challenge exponential backoff on
  failed attempts; the current attempt cap plus TTL is considered sufficient
  for the accepted risk threshold.
