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
