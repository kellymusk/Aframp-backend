# Security Notes

## JWT revocation on account deletion

When a user account is deleted via `src/services/users.rs` → `anonymize_and_delete()`, the
user row is anonymized and `deleted_at` is set. Setting `deleted_at` causes
`session_is_valid` to return `false` for any subsequent request, which locks the account.

To defend in depth against any path that might short-circuit or cache `session_is_valid`,
`anonymize_and_delete()` also bulk-inserts every unexpired `jti` belonging to the user into
the `revoked_tokens` table (the same table used by `src/auth/jwt.rs` → `revoke()`). Any
in-flight JWT held by the client — or a copy on another device — is therefore rejected with
`401 Unauthorized` once the account is deleted.

### Legacy-token gap

Tokens issued before the `jti` claim was introduced carry `jti = None`. Because they have no
identifier, they can never be inserted into `revoked_tokens` and therefore cannot be revoked
by logout or by account deletion. Such tokens remain valid until they expire naturally.

Mitigations:

- Keep JWT lifetimes short so the exposure window for legacy tokens is bounded.
- Rely on `session_is_valid` (which checks `deleted_at`) as the primary gate for these tokens.
- Rotate the signing key if a legacy token is known to be compromised; this invalidates all
  outstanding tokens at once.

## Login email enumeration protection

`src/api/auth.rs` → `login()` must not leak whether an email address is registered. Two
properties are enforced and covered by tests:

### Uniform rate-limit accounting

`src/services/login_rate_limit.rs` → `hit()` is invoked for **every** `/login` attempt,
before and independently of the user lookup. The counter is keyed by client IP and is
incremented identically whether the submitted email exists or not. As a result, an attacker
cannot distinguish valid from invalid emails by observing how quickly the rate limit trips:
the threshold is the same for both cases. The IP-based limit is checked first (correct for
DoS protection) and the accounting path never branches on email existence.

### Timing-safe credential check

When the email is unknown, `login()` still performs a password hash verification against a
fixed dummy hash before returning the generic `401 Unauthorized` response. This keeps the
response timing for "unknown email" and "wrong password" indistinguishable at the HTTP
level, so timing measurements cannot be used to enumerate accounts. All failure modes return
the same status and body.
