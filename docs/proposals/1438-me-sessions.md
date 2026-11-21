# Proposal: `GET /me/sessions` and `DELETE /me/sessions/{jti}` (#1438)

## Problem
Merchants can't see or revoke individual sessions (e.g. a lost device). `revoked_tokens` stores revoked `jti`s, but nothing tracks issued tokens per user.

## Data model
New table `user_sessions`:

| Column | Type | Notes |
|---|---|---|
| `jti` | text PK | Matches the JWT `jti` claim |
| `user_id` | uuid FK → users | Indexed |
| `issued_at` | timestamptz | |
| `expires_at` | timestamptz | Rows past this are pruned |
| `user_agent` | text null | From login request |
| `ip` | inet null | From login request |

A row is inserted when `src/auth/jwt.rs` issues a token (login, verify-otp, refresh). Active = row exists, `expires_at > now()`, and `jti` not in `revoked_tokens`.

## Endpoints
- `GET /me/sessions` → `200` array of `{ jti, issued_at, expires_at, user_agent, ip, current }`.
- `DELETE /me/sessions/{jti}` → inserts into `revoked_tokens`, `204`. `404` if the `jti` isn't an active session owned by the caller.

## Security
- Scope every query by `user_id` from the authenticated token; never reveal whether another user's `jti` exists (return `404`).
- Revoking the current session behaves like `POST /logout`.
- `DELETE /me` and password change revoke all rows.
- Rate-limit `DELETE` like other authenticated writes.
- Store IP/user agent only for display; prune with expired rows.

## Tests (to add in `tests/`)
List returns the current session; revoke returns `204`; a request with the revoked `jti` returns `401`; another user's `jti` returns `404`.
