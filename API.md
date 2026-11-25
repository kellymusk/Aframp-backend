# Aframp Pay — API reference

Everything documented here is implemented and covered by integration tests against a real Postgres database. Where behaviour is partial or stubbed, it's marked inline rather than omitted.

- **Base URL (dev):** `http://127.0.0.1:3000`
- **Content type:** `application/json` on every request with a body
- **Request body limit:** 64KB (65,536 bytes) by default, configurable server-side via `MAX_REQUEST_BODY_BYTES`. Larger bodies are rejected with `413 Payload Too Large` before reaching any handler. Every current endpoint needs well under 1KB.
- **Machine-readable spec:** [`openapi.yaml`](openapi.yaml) — generate a typed client from it rather than hand-writing calls

---

## Authentication

There are two ways to authenticate. **Browsers should use the cookie.**

### Session cookie (browsers)

`/signup` and `/login` set an `HttpOnly` session cookie:

```
Set-Cookie: aframp_session=<jwt>; HttpOnly; Path=/; SameSite=Lax; Max-Age=86400; Secure
```

Send subsequent requests with `credentials: 'include'` (or nothing at all if the frontend is served same-origin) and the browser attaches it for you. `POST /logout` clears it.

**Do not store the JWT in `localStorage`.** The token is still echoed in the response body for API clients, but a browser that copies it into `localStorage` hands the whole session to any XSS on the page — which is exactly what the `HttpOnly` cookie exists to prevent. Ignore the `token` field; read the login response only for `user_id` and `merchant_id`.

### Bearer token (API clients, scripts, tests)

Non-browser clients send the JWT from the response body as a header:

```
Authorization: Bearer <token>
```

The header takes precedence when both are present.

Tokens are **HS256, valid for 24 hours** either way. Claims are `sub` (user id), `merchant_id`, `iat`, `exp`, and `orig_iat` (when the session was first issued, kept across refreshes).

Two things worth building for up front:

- **`merchant_id` is nullable.** `AuthResponse.merchant_id` and the JWT claim are both optional. Today signup always creates a merchant so it's always present, but the type allows `null` — an account without a merchant gets `400` from every merchant-scoped endpoint, not `401`. Don't assume non-null.
- **Refresh before expiry.** `POST /auth/refresh` swaps a still-valid token for a new 24h one, for up to 7 days from the original login. Once a token has expired (or the 7 days are up), calls return `401` with `{"error":"invalid or expired token","code":"INVALID_CREDENTIALS"}` — treat any `401` on a previously-working call as "send the user back to login."

### CORS

Browser origins must be allowlisted server-side via the `CORS_ALLOWED_ORIGINS` env var (comma-separated, defaults to `http://localhost:3001`). Allowed methods are `GET`, `POST`, `PUT`, `PATCH` and `DELETE` (`PATCH`/`DELETE` are listed proactively so new endpoints don't fail preflight — see #1249); allowed headers are `Authorization` and `Content-Type`. Credentials **are** enabled, so the origin list is never mirrored back — an origin that isn't listed fails preflight.

The supported deployment is **same-origin**: serve the frontend and this API behind one hostname (the reverse proxy routes `/api/*` here) and CORS stops applying at all. Cross-origin cookie auth additionally needs `COOKIE_SAME_SITE=none`, which lets the session ride cross-site requests and reintroduces CSRF as something you have to handle. With the default `SameSite=Lax`, a cross-origin frontend won't get the cookie sent at all and has to fall back to the bearer header.

---

### `AuthUser` vs `AuthMerchant`

- **`AuthUser`** — any authenticated session. Used by account-level routes
  (e.g. `/me`).
- **`AuthMerchant`** — an authenticated session whose user has an associated
  merchant. Merchant-scoped routes (withdrawals, payment requests, wallets,
  transactions) require this; a user with no merchant gets `400
  MERCHANT_NOT_FOUND`.

## Authentication Flow

Sessions are **always two-step** for any account with a verified phone number. Neither `/signup` nor `/login` issues a session — they only fire an OTP. The session is issued exclusively by `POST /verify-otp`.

### Signup flow

```mermaid
sequenceDiagram
    participant C as Client
    participant A as Aframp API
    participant T as Termii (SMS)

    C->>A: POST /signup<br>{ email, password, name, phone_number }
    A->>A: Validate fields (email format, password ≥ 8 chars,<br>phone parses to E.164, not already registered)
    A->>T: Send OTP to phone_number
    A-->>C: 200 { challenge_id, expires_in_secs: 600 }
    Note over C: No session yet.<br>Store challenge_id; show code-entry UI.

    C->>A: POST /verify-otp<br>{ challenge_id, code }
    A->>A: Validate code (10-min window, 5-attempt limit)
    A->>A: INSERT user + merchant (transactional)
    A-->>C: 200 { token, user_id, merchant_id }<br>Set-Cookie: aframp_session=…
    Note over C: Session active. Ignore token<br>in localStorage — use the cookie.
```

**Resend:** re-`POST` to `/signup` with the same email and credentials. There's no separate resend endpoint. Rate limits apply: one send per 60 seconds per phone, and at most 5 sends per hour per phone (`429 TOO_MANY_REQUESTS` if you exceed either).

**Error states during verify-otp:**

| Code | Meaning | What to do |
|---|---|---|
| `OTP_INVALID` | Wrong code (up to 5 attempts before lockout) | Show "wrong code", offer resend |
| `OTP_EXPIRED` | Code older than 10 minutes | Re-POST `/signup` for a new challenge |
| `OTP_LOCKED` | 5 wrong attempts — challenge is dead | Re-POST `/signup` for a new challenge |
| `OTP_CHALLENGE_NOT_FOUND` | Unknown or already-consumed `challenge_id` | Re-POST `/signup` |
| `EMAIL_TAKEN` / `PHONE_TAKEN` | Another account verified between signup and verify | Show conflict message |

---

### Login flow

```mermaid
sequenceDiagram
    participant C as Client
    participant A as Aframp API
    participant T as Termii (SMS)

    C->>A: POST /login<br>{ email, password }
    A->>A: Verify password (constant-time;<br>same 401 for wrong password or unknown email)

    alt Account has a verified phone number (normal case)
        A->>T: Send fresh OTP to phone_number
        A-->>C: 200 { challenge_id, expires_in_secs: 600 }
        Note over C: No session yet.<br>Show code-entry UI.

        C->>A: POST /verify-otp<br>{ challenge_id, code }
        A->>A: Validate code
        A-->>C: 200 { token, user_id, merchant_id }<br>Set-Cookie: aframp_session=…
    else Legacy account — no phone on file (pre-OTP rollout only)
        A-->>C: 200 { token, user_id, merchant_id }<br>Set-Cookie: aframp_session=…
        Note over C: One-step login, no OTP.<br>Only possible for accounts<br>created before OTP existed.
    end
```

**Wrong credentials:** `/login` returns `401 INVALID_CREDENTIALS` for both a wrong password and an unknown email — deliberately identical, so you cannot tell the difference and neither can an attacker.

**Resend during login:** re-`POST` to `/login` with the same credentials. Same rate limits as signup (60 s cooldown, 5 per hour per phone).

---

### Rate limits at a glance

| Action | Limit | Error | Header |
|---|---|---|---|
| OTP send per phone | 1 per 60 seconds | `429 TOO_MANY_REQUESTS` | `Retry-After: <secs>` (how long to wait, ≤ 60) |
| OTP send per phone | 5 per hour | `429 TOO_MANY_REQUESTS` | `Retry-After: <secs>` |
| Wrong OTP guesses | 5 per challenge | `400 OTP_LOCKED` (challenge dead) | — |
| Code validity window | 10 minutes | `400 OTP_EXPIRED` | — |

When a `429` response is returned, the `Retry-After` header value is the number
of **whole seconds** the client should wait before retrying. Always read this
header; do not hard-code a back-off duration on the client side.

```js
if (res.status === 429) {
    const retryAfter = parseInt(res.headers.get('Retry-After') ?? '60', 10);
    scheduleRetry(retryAfter * 1000); // back off for exactly this long
}
```

---

## Errors

Every error returns the same shape — a human-readable `error` string plus a stable, machine-readable `code` you can branch on:

```json
{ "error": "insufficient available balance", "code": "INSUFFICIENT_BALANCE" }
```

`code` is stable and never changes wording; `error` is written for humans. Match on `code`, never on the `error` string.

| Status | Meaning | Frontend handling |
|---|---|---|
| `400` | Validation failed, or the account has no merchant | Show the `error` string; it's written for humans |
| `415` | `Content-Type` isn't `application/json` on a POST/PUT with a body | Send `Content-Type: application/json` and retry |
| `401` | Missing, malformed, or expired token | Redirect to login |
| `404` | Resource not found | — |
| `409` | Email or phone already registered | Show on the signup form |
| `403` | Authenticated, but not an admin (`/admin/*` only) | Not applicable to merchant-facing routes |
| `429` | OTP resent too soon, or too many times this hour | Read `Retry-After` header (seconds); show the wait; don't auto-retry |
| `502` | Upstream payment provider failed (`PAYOUT_FAILED`) | **Do not retry the same withdrawal.** The withdrawal row is already created (then marked `failed` and the balance refunded). Show the `error` string; let the user start a *new* withdrawal if they want to try again. |
| `500` | Internal error | Generic `INTERNAL_ERROR`; details stay in server logs |

### Error code catalog

Every `ErrorCode` variant defined in `src/error.rs` is listed below, with the HTTP status it travels with and the endpoints that can emit it. Match on `code`; the `error` string is for humans and may change.

| Code | HTTP Status | Description | Endpoints that can return it |
|---|---|---|---|
| `INVALID_PARAMETERS` | `400` | A required field is missing or malformed (short password, empty name/email, unparseable phone, bad `bank_code`/`account_number`, malformed body) | `POST /signup`, `POST /login`, `POST /verify-otp`, `POST /wallet/create`, `POST /payment-requests`, `POST /withdraw` |
| `INVALID_AMOUNT` | `400` | Amount is not positive, or not a whole number of kobo (`amount_stroops` not a multiple of `100000`) | `POST /payment-requests`, `POST /withdraw` |
| `INSUFFICIENT_BALANCE` | `400` | Withdrawal exceeds the merchant's available balance | `POST /withdraw` |
| `UNSUPPORTED_ASSET` | `400` | Withdrawal `asset` isn't `cNGN` | `POST /withdraw` |
| `PAYOUT_FAILED` | `502` | Upstream payment provider (Paystack) rejected the payout; the balance is refunded and the withdrawal is recorded as `failed` | `POST /withdraw` |
| `EMAIL_TAKEN` | `409` | Signup email already registered to a verified account | `POST /signup` |
| `PHONE_TAKEN` | `409` | Signup phone already registered to a verified account | `POST /signup` |
| `INVALID_CREDENTIALS` | `401` | Wrong password or unknown email on login; also any missing, malformed, or expired token on an authenticated route | `POST /login`, every auth-required endpoint |
| `USER_NOT_FOUND` | `404` | Authenticated user no longer exists | Any auth-required endpoint (`GET /me`, `POST /wallet/create`, `GET /wallet`, `POST /payment-requests`, `GET /payment-requests`, `GET /balance`, `GET /transactions`, `POST /withdraw`, `GET /withdrawals`) |
| `MERCHANT_NOT_FOUND` | `400` | Account has no merchant — send the user through onboarding | Any merchant-scoped endpoint (`GET /me`, `POST /wallet/create`, `GET /wallet`, `POST /payment-requests`, `GET /payment-requests`, `GET /balance`, `GET /transactions`, `POST /withdraw`, `GET /withdrawals`) |
| `WALLET_NOT_FOUND` | `400` | No wallet yet, or none created before a payment-request call | `GET /wallet`, `POST /payment-requests` |
| `PAYMENT_REQUEST_NOT_FOUND` | `404` | Payment request id doesn't exist | `GET /payment-requests/{id}` |
| `FORBIDDEN` | `403` | Authenticated but not an admin, on an `/admin/*` route | `/admin/*` routes |
| `OTP_INVALID` | `400` | Wrong code submitted to `/verify-otp` | `POST /verify-otp` |
| `OTP_EXPIRED` | `400` | Code submitted after its 10-minute window | `POST /verify-otp` |
| `OTP_CHALLENGE_NOT_FOUND` | `404` | Unknown or already-consumed `challenge_id` | `POST /verify-otp` |
| `OTP_LOCKED` | `400` | 5 wrong attempts on this challenge — request a new one via `/signup` or `/login` | `POST /verify-otp` |
| `TOO_MANY_REQUESTS` | `429` | OTP resent inside the 60s cooldown, or 5th+ send this hour. The response includes a `Retry-After` header (integer seconds) indicating how long to wait before retrying. | `POST /signup`, `POST /login` |
| `INTERNAL_ERROR` | `500` | Unexpected server error; generic message only | Any endpoint |

> **`INTERNAL_ERROR` is always a `500`.** The response body carries the fixed, safe message `"internal server error"` — never a stack trace, SQL error, or any other internal detail. Those are written to the server logs only. Treat a `500` as "something broke on our side, retry or report it," and never try to parse the `error` string for a cause.
| Code | Status | When it's returned |
|---|---|---|
| `INVALID_PARAMETERS` | `400` | A required field is missing/malformed (e.g. short password, bad account_number/bank_code) |
| `INVALID_AMOUNT` | `400` | Amount is not positive, or not a whole number of kobo |
| `INSUFFICIENT_BALANCE` | `400` | Withdrawal exceeds the available balance |
| `UNSUPPORTED_ASSET` | `400` | Withdrawal asset isn't cNGN |
| `EMAIL_TAKEN` | `409` | Signup email already registered (to a verified account) |
| `PHONE_TAKEN` | `409` | Signup phone already registered (to a verified account) |
| `INVALID_CREDENTIALS` | `401` | Wrong password or unknown email on login |
| `USER_NOT_FOUND` | `404` | Authenticated user no longer exists |
| `MERCHANT_NOT_FOUND` | `400` | Account has no merchant (visit onboarding) |
| `WALLET_NOT_FOUND` | `404` | No wallet yet, or none created before a payment-request call |
| `PAYMENT_REQUEST_NOT_FOUND` | `404` | Payment request id doesn't exist |
| `PAYOUT_FAILED` | `502` | Upstream payment provider rejected the payout. The withdrawal audit row already exists (`status: failed`); balance was refunded. **Do not auto-retry** — retrying creates a duplicate attempt. |
| `FORBIDDEN` | `403` | Authenticated but not an admin, on an `/admin/*` route |
| `OTP_INVALID` | `400` | Wrong code submitted to `/verify-otp` |
| `OTP_EXPIRED` | `400` | Code submitted after its 10-minute window |
| `OTP_LOCKED` | `400` | 5 wrong attempts on this challenge — request a new one |
| `OTP_CHALLENGE_NOT_FOUND` | `404` | Unknown or already-consumed `challenge_id` |
| `TOO_MANY_REQUESTS` | `429` | OTP resent inside the 60s cooldown, or 5th+ send this hour |
| `INTERNAL_ERROR` | `500` | Unexpected server error; generic message only |

---

## Rate Limiting

`429 Too Many Requests` (rate limit / cooldown) and `423 Locked` (temporary
lockout) responses carry a `retry_after_secs` field in the JSON body, and the
server mirrors it as a `Retry-After` header (seconds).

Recommended client behaviour: wait `Retry-After` seconds before retrying; if the
header is missing, fall back to `retry_after_secs`; if both are missing, use
exponential backoff with jitter.

```js
const res = await fetch(url, opts);
if (res.status === 429 || res.status === 423) {
  let wait = Number(res.headers.get("Retry-After"));
  if (!wait) wait = (await res.json()).retry_after_secs ?? 1;
  await new Promise(r => setTimeout(r, wait * 1000));
}
```

## Conventions

**Money is always integer stroops** (`i64`), never a float. 1 unit = `10_000_000` stroops.

```js
const toDisplay = (stroops) => (stroops / 10_000_000).toFixed(7);
const toStroops = (amount) => Math.round(amount * 10_000_000);
```

Conversion reference:

| XLM | Stroops |
|---|---|
| 0.0000001 | 1 (minimum) |
| 1.0000000 | 10,000,000 |
| 2.5000000 | 25,000,000 |
| 50.0000000 | 500,000,000 |
| 100.0000000 | 1,000,000,000 |

Never use floating-point arithmetic to accumulate balances — convert for display only.

**Timestamps** are RFC 3339 / ISO 8601 UTC (`2026-08-13T14:15:34.520195Z`), parseable by `new Date()`.

**Ids** are UUID v4 strings.

---

## Amounts and Denominations

All monetary values in this API are integers denominated in **stroops** — the
smallest unit of Stellar assets. Never send whole units.

- `1 XLM = 10,000,000 stroops`
- `1 cNGN = 10,000,000 stroops`
- `1 cNGN stroop = 100,000 kobo` (i.e. `1 cNGN` = `100 kobo`)

Conversion formula:

```
amount_stroops = whole_units × 10,000,000
```

| Whole units | `amount_stroops` |
|---|---|
| 0.0000001 XLM | 1 (minimum) |
| 1 XLM / 1 cNGN | 10,000,000 |
| 2.5 XLM | 25,000,000 |
| 50 cNGN | 500,000,000 |
| 2,500 cNGN (₦2,500) | 25,000,000,000 |

Example: `₦2,500 = 25,000,000,000 cNGN stroops`.

> **Warning:** the most common integration mistake is sending amounts in whole
> units (e.g. `25`) instead of stroops (`250,000,000`). That creates a payment
> for `0.0000025` units. Always multiply by `10,000,000` before sending, and
> never use floating-point arithmetic to accumulate balances — convert for
> display only.

```js
const toDisplay = (stroops) => (stroops / 10_000_000).toFixed(7);
const toStroops = (amount) => Math.round(amount * 10_000_000);
```

---

## Pagination

`GET /transactions`, `GET /payment-requests` and `GET /withdrawals` return
`{ "data": [...], "next_cursor": "<token>" | null }`, newest first. To get the
next page, repeat the request with `?cursor=<next_cursor>` (same `limit`);
`next_cursor` is `null` on the last page. The cursor is an opaque token — don't
parse or build it. Pages are keyed on `(created_at, id)`, so rows created while
you page never shift or repeat later pages. A malformed cursor returns `400`.

Cursor handling is identical on every list endpoint: omit `cursor` for the
first page; a present but malformed or tampered cursor is always rejected with
the same `400` error body, regardless of endpoint.

### `limit` behaviour

Every list endpoint accepts `?limit=` (default **50**). The value is **clamped**, not rejected:

| You send | You get |
|---|---|
| `limit=10` | 10 rows |
| `limit=0` or a negative value | 1 row (clamped up to the minimum) |
| `limit=200` | 200 rows |
| `limit=99999` | 200 rows (clamped down to the maximum) — **no error, no indication the value was ignored** |

So a client that asks for `limit=99999` receives a normal `200` with at most 200 rows and has no way to tell its value was capped. Treat `next_cursor` as the only signal that more data exists.

> **Planned change (#1126):** out-of-range `limit` values will be rejected with `400 INVALID_PARAMETERS` and a message naming the allowed range, instead of being silently clamped. Until that lands, clamp client-side and don't rely on the server to report the effective limit. See [`docs/proposals/1126-query-limit-error-shape.md`](docs/proposals/1126-query-limit-error-shape.md).

## Endpoints

### `GET /health`
Liveness probe. No auth. Returns `200` with `{"status": "ok", "version": "<crate version>"}`.

### `GET /`
Returns the literal string `aframp` (not JSON). Useful as a smoke test.

---

### `POST /signup`
No auth. **Does not create the account.** Validates the credentials, sends an OTP to `phone_number`, and returns a challenge — the user + merchant only get inserted once `POST /verify-otp` succeeds with the right code. This means the same email can be re-submitted freely (it's the resend path) as long as no prior attempt for it was ever verified.

```json
{ "email": "merchant@example.com", "password": "at-least-8-chars", "name": "Shop Name", "phone_number": "08011122233" }
```

| Field | Limits |
|---|---|
| `email` | Valid address shape; max **254** characters (RFC 5321). Local-part ≤ 64, domain ≤ 255. |
| `password` | Min 8 characters |

**Password character rules:** length is counted in Unicode characters, not bytes. The uppercase and digit checks are ASCII-only (`A–Z`, `0–9`); `Ä` does not count as uppercase. Any non-alphanumeric character counts as "special", but Unicode letters such as `ñ` or `é` are alphanumeric, so they don't satisfy that rule. Non-ASCII characters are otherwise accepted; note they take more bytes (and Argon2 hashing work) per visible character.
| `name` | Non-empty after trim; max **100** characters (validated before the password is hashed) |
| `phone_number` | Nigerian mobile; normalized to E.164 (`+234…`, 13 chars). Oversized / non-NG input is rejected. |

`phone_number` accepts any common Nigerian mobile format (`0801...`, `801...`, `+234801...`, `234801...`) and is normalized to E.164 before storage/sending.

`200` →
```json
{ "challenge_id": "b6b54b1e-...", "expires_in_secs": 600 }
```

Errors: `400` if email is empty/invalid/oversized, password is under 8 characters, name is empty or longer than 100 characters, or the phone doesn't parse. `409` (`EMAIL_TAKEN`/`PHONE_TAKEN`) if either is already registered to a *verified* account. `429` (`TOO_MANY_REQUESTS`) if resent within 60s of the last send, or 5 times inside an hour.

### `POST /login`
No auth. Verifies the password. If the account has a verified phone number, this returns a **fresh OTP challenge** (identical shape to `/signup`'s) instead of a session — full two-factor, every login, no exception for a returning session. Only an account with no phone on file (impossible to create anymore; a relic of accounts made before this existed) logs in synchronously with the old one-step response.

```json
{ "email": "merchant@example.com", "password": "at-least-8-chars" }
```

`200` → either `{ "challenge_id": "...", "expires_in_secs": 600 }` (the normal case) or the full session response described under `/verify-otp` below (legacy accounts only).

Errors: `401` for both a wrong password and an unknown email — deliberately indistinguishable, so don't build a "no such account" message from it. `429` on the same resend rules as signup.

**Rate limit.** Every `/login` attempt counts against two fixed 15-minute windows, stored in Postgres (`login_attempts`) so they hold across restarts and instances:

| Key | Limit per 15 min |
|---|---|
| Email address (case-insensitive) | 5 attempts |
| Client IP (TCP peer address) | 20 attempts |

Going over either returns `429` with `code: "TOO_MANY_REQUESTS"` and a `Retry-After` header giving the seconds until the window resets. A correct password resets that email's counter. Counters expire on their own when their window ends. Behind a reverse proxy the peer address is the proxy's, so the per-IP limit then applies to all traffic through it; the per-email limit still applies.

### `POST /verify-otp`
No auth. The **only** endpoint that ever issues a session, reached from either a signup or a login challenge.

```json
{ "challenge_id": "b6b54b1e-...", "code": "482913" }
```

`200` →
```json
{
  "token": "eyJ0eXAiOiJKV1Qi...",
  "user_id": "2c5e0ee2-7f87-4efb-b1c9-d7e1b3ee0eeb",
  "merchant_id": "6a91d75c-8c41-4fa5-b10b-6eb8cda8ac0a"
}
```
…with the same `Set-Cookie: aframp_session=...` as before. For a signup challenge, the user and merchant are created transactionally at this exact moment, not before.

Errors: `400` `OTP_INVALID` (wrong code — 5 wrong guesses and the challenge is dead, not just that attempt), `OTP_EXPIRED` (codes last 10 minutes), `OTP_LOCKED` (attempts exhausted — restart via `/signup` or `/login` for a new one). `404` `OTP_CHALLENGE_NOT_FOUND` for an unknown or already-consumed `challenge_id`.

### `POST /auth/refresh`
Auth required (bearer or session cookie; the token must not have expired). Returns a new token with the same body shape as `/verify-otp` and resets the session cookie. The new token expires 24h from now, but never more than 7 days after the original login; past that point this returns `401` ("session can no longer be refreshed; log in again"). Refresh doesn't revoke the old token — it stays valid until its own `exp`.

### `POST /auth/change-password`
Auth required (bearer or session cookie).

```json
{ "current_password": "old-password", "new_password": "new-password-123" }
```

Re-proves the current password, then replaces it. `200` → a fresh session with the same body shape as `/verify-otp`, also set as the session cookie, so the device that made the change stays signed in. **Every other session for the account stops working immediately** — including ones that were refreshed via `/auth/refresh`, since refreshes keep the original login time.

Errors: `400` with `field: "new_password"` if it's shorter than 8 characters; `400` with `field: "current_password"` if the current password is wrong (deliberately not `401`, which clients treat as "session expired"); `401` for a missing, expired or already-revoked session.
### `GET /verify-email`
No auth. Consumes the single-use, 24-hour token from the verification email
sent when a signup completes or a resend is requested.

```
GET /verify-email?token=<hex-token>
```

`200` → `{ "email_verified": true }`. Unknown, already-used, or expired tokens
return `400` `INVALID_PARAMETERS` ("invalid or expired email verification
token"), and an empty `token` returns `400` with `field: "token"`.

### `POST /verify-email/resend`
Auth required. Issues a fresh verification link for the signed-in account,
invalidating any previous token. Returns `200` `{ "sent": true }`. Because no
transactional email provider is wired into the backend yet, the link is emitted
through structured logs (`verification_link`).

### `POST /logout`
No auth — a browser holding an expired or malformed session still needs to clear it. Returns `204` and a `Set-Cookie` that expires `aframp_session` immediately.

It also revokes the presented token (the `Authorization: Bearer` header, or the session cookie if no header is sent): the token's `jti` goes on a server-side revocation list until the token would have expired, so a copy of it held elsewhere is rejected with `401` from then on. Other sessions of the same user are unaffected.

### `GET /me`
Auth required. The signed-in user's profile. The JWT carries only ids, so call this after a reload to render anything human-readable without forcing a re-login.

`200` →
```json
{
  "user_id": "2c5e0ee2-7f87-4efb-b1c9-d7e1b3ee0eeb",
  "email": "merchant@example.com",
  "name": "Shop Name",
  "phone_number": "+2348011122233",
  "phone_verified": true,
  "created_at": "2026-08-13T14:15:34.232320Z",
  "merchant_id": "6a91d75c-8c41-4fa5-b10b-6eb8cda8ac0a",
  "merchant_name": "Shop Name"
}
```

`merchant_id` and `merchant_name` are `null` for an account with no merchant. The password hash is never serialized.
`phone_number` is `null` for legacy accounts without a number; `phone_verified` is `false` until phone verification succeeds.

---

### `GET /merchants/me`
Auth required. The signed-in user's merchant profile on its own, separate from `GET /me`. Returns the full `Merchant` model so richer business data (address, logo, bank account, verification status) can grow here without bloating the auth-identity endpoint.

`200` →
```json
{
  "id": "6a91d75c-8c41-4fa5-b10b-6eb8cda8ac0a",
  "user_id": "2c5e0ee2-7f87-4efb-b1c9-d7e1b3ee0eeb",
  "name": "Shop Name",
  "email": "merchant@example.com",
  "email_notifications_enabled": true,
  "unsubscribe_token": null,
  "suspended_at": null,
  "created_at": "2026-08-13T14:15:34.232320Z",
  "updated_at": "2026-08-13T14:15:34.232320Z"
}
```
`404` with `MERCHANT_NOT_FOUND` when the account has no merchant profile. `email`, `unsubscribe_token` and `suspended_at` are `null` when unset.

---

### `PATCH /me`
Auth required. Body: `{ "name"?: string, "phone_number"?: string }` (at least one). A new `name` (trimmed, 1–100 characters) applies immediately. A new `phone_number` (any common Nigerian format, normalized to `+234…`) is **not** switched yet: an OTP is sent to it and the response includes `phone_verification: { challenge_id, expires_in_secs }`. Complete it with `POST /verify-otp` (as at signup) to move the account to the new number.

`200` → `{ "name": "...", "phone_number": "<number on file>", "phone_verification": null | {...} }`. `400` with `field` for invalid input, `409` if the number belongs to another account, `429` if OTP sends to that number are rate limited.

### `PATCH /users/me`
Alias of `PATCH /me` for user name (and phone) changes; same body, rules and response.

### `PATCH /merchants/me`
Auth required (merchant account). Body: `{ "name"?: string, "email"?: string }` (at least one). A new `name` (trimmed, 1–100 characters, same rules as `validate_name`) applies immediately. A new `email` is **not** switched yet: it is stored as `pending_email` and a confirmation token is sent to the new address (valid for 30 minutes).

`200` → `{ "merchant_id": "...", "name": "...", "email": "<address on file>" | null, "pending_email": null | "..." }`. `400` with `field` for invalid input.

### `POST /merchants/me/email/confirm`
Auth required. Body: `{ "token": string }`. Moves `pending