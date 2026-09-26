# ADR-003: HMAC-based OTP code storage (not bcrypt/Argon2)

**Status:** Accepted  
**Date:** 2024-11-01  
**Deciders:** Core team

---

## Context

When a 6-digit OTP is created it must be stored in some form so that the
`/verify-otp` endpoint can confirm the code the user submits. Three options
were considered:

**Option A — Store the plain code**  
Write the code directly to the `otp_challenges.code` column. Simple, but means
anyone with database read access (or who sees a backup) can read outstanding
codes and log in as any pending-signup or logged-out user.

**Option B — Hash with bcrypt or Argon2**  
Hash the code the same way passwords are hashed. Protects against DB
exposure, but these algorithms are *intentionally slow*. With only 1 000 000
possible values, a single offline dictionary attack (compute all one million
hashes, build a lookup table) takes seconds to set up and forever to exploit —
the cost parameter buys almost nothing against a full precomputation of the
search space.

**Option C — HMAC-SHA256 keyed with a server secret**  
Compute `HMAC-SHA256(key=OTP_HMAC_SECRET, input=challenge_id || code)` and
store the hex digest. The secret lives in the environment, not the database. An
attacker who can read the database but not the environment variable cannot
reverse the stored hash — they lack the key.

---

## Decision

**Option C (HMAC-SHA256) was chosen.**

The implementation is in `src/services/otp.rs` (`hash_code` /
`verify_code` functions). Key properties:

- The HMAC key is `OTP_HMAC_SECRET`, which must live outside the database (env
  var / secret manager).
- The `challenge_id` (a UUID) is folded into the MAC input alongside the code.
  This means the same 6-digit code produces a different stored hash for every
  challenge row — a trivial defense-in-depth measure given that lookups are
  already scoped `WHERE id = $challenge_id`.
- Verification is a constant-time MAC comparison (`mac.verify_slice`), not a
  string equality check.

The choice is explained inline in the source:

> *A 6-digit code has only 1,000,000 possible values, so a bare hash of it —
> even salted — is trivially reversible by anyone with DB read access
> (precompute all million hashes once, look up forever; the search space is the
> bottleneck, not precomputation). HMAC keyed with a secret that lives outside
> the database is what actually stops that.*

---

## Consequences

**Positive:**

- A database dump without `OTP_HMAC_SECRET` reveals no exploitable code
  information — the stored digest is useless to an attacker who doesn't hold
  the key.
- HMAC-SHA256 is constant-time by construction (via the `hmac` crate's
  `verify_slice`), avoiding timing side-channels.
- Verification is fast (microseconds) — no UX impact from hash stretching.

**Negative / open items:**

- Security depends entirely on `OTP_HMAC_SECRET` remaining secret. If it leaks
  alongside the database, an attacker can precompute the full space in seconds.
  The secret must be rotated promptly if compromise is suspected; outstanding
  challenges will be invalidated (they will fail verification), which is the
  correct safe-fail behaviour.
- `OTP_HMAC_SECRET` must be distinct from `JWT_SECRET` and
  `WALLET_ENCRYPTION_KEY`. Reusing any secret across multiple purposes
  undermines isolation.
- This choice is appropriate for short-lived, single-use codes. **Do not apply
  this pattern to long-lived credentials** (passwords, API keys) — those
  require a memory-hard KDF (Argon2, bcrypt, scrypt).
