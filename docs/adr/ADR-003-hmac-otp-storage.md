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

## Why 6 decimal digits, and the online brute-force risk

Six decimal digits were chosen as a deliberate UX/security trade-off:

- **Keyspace:** `generate_code()` produces a uniformly random value in
  `0..=999_999`, i.e. **1,000,000 possible codes**.
- **Usability:** 6 digits is the de-facto standard for SMS/email OTPs — short
  enough to type on any device, long enough that a single blind guess is
  hopeless. Longer codes (8+) measurably increase mistyped-entry and
  abandonment rates for no meaningful gain once online guessing is throttled.
- **Online guessing is the only realistic attack** (offline attacks are already
  defeated by the HMAC storage above). The relevant controls are:

| Control | Value | Effect on online guessing |
| --- | --- | --- |
| `MAX_ATTEMPTS` | 5 per challenge | Caps guesses per challenge at 5 |
| `CODE_TTL_SECS` | 600 s (10 min) | Bounds the window a challenge is live |
| Per-IP login rate limiting | existing middleware | Bounds challenges an attacker can create/attack per IP |

**Per-challenge brute-force probability.** With 5 attempts against a
1,000,000-value space, the chance a single challenge is guessed is at most
`5 / 1,000,000 = 0.0005%` (5e-6). This is the figure documented by the test
`brute_force_probability_is_bounded` in `src/services/otp.rs`.

**Acceptable risk threshold.** We accept a per-challenge success probability of
**≤ 0.001% (1e-5)**. The current design sits at 5e-6, comfortably under that
bound. If `MAX_ATTEMPTS` were raised, or the keyspace reduced, the threshold
would be breached and this ADR must be revisited.

**Residual risk at scale.** An attacker who knows many `challenge_id`s (they are
public UUIDs returned by signup/login) could spread 5 guesses across many
challenges. This is why the per-IP login rate limiting matters: it bounds how
many challenges can be minted and attacked from one source. If abuse is
observed, the follow-up mitigations are (a) a shorter resend cooldown, and
(b) per-challenge attempt rate limiting with exponential backoff — both noted
as open items below.

---

## Consequences

**Positive:**

- A database dump without `OTP_HMAC_SECRET` reveals no exploitable code
  information — the stored digest is useless to an attacker who doesn't hold
  the key.
- HMAC-SHA256 is constant-time by construction (via the `hmac` crate's
  `verify_slice`), avoiding timing side-channels.
- Verification is fast (microseconds) — no UX impact from hash stretching.
- Online brute-force risk is bounded and documented (≤ 0.001% per challenge).

**Negative / open items:**

- Security depends entirely on `OTP_HMAC_SECRET` remaining secret. If it leaks
  alongside the database, an attacker can precompute the full space in seconds.
  The secret must be rotated promptly if compromise is suspected; outstanding
  challenges will be invalidated (they will fail verification), which is the
  correct safe-fail behaviour.
- `OTP_HMAC_SECRET` must be distinct from `JWT_SECRET` and
  `WALLET_ENCRYPTION_KEY`. Reusing any secret across multiple purposes
  undermines isolation.
- **Open item:** consider reducing the resend cooldown further and/or adding
  per-challenge attempt rate limiting with exponential backoff to harden the
  multi-challenge attack surface described above.
- This choice is appropriate for short-lived, single-use codes. **Do not apply
  this pattern to long-lived credentials** (passwords, API keys) — those
  require a memory-hard KDF (Argon2, bcrypt, scrypt).
