# Aframp Pay — API reference

Everything documented here is implemented and covered by integration tests against a real Postgres database. Where behaviour is partial or stubbed, it's marked inline rather than omitted.

- **Base URL (dev):** `http://127.0.0.1:3000`
- **Content type:** `application/json` on every request with a body
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

Browser origins must be allowlisted server-side via the `CORS_ALLOWED_ORIGINS` env var (comma-separated, defaults to `http://localhost:3001`). Allowed methods are `GET`/`POST`; allowed headers are `Authorization` and `Content-Type`. Credentials **are** enabled, so the origin list is never mirrored back — an origin that isn't listed fails preflight.

The supported deployment is **same-origin**: serve the frontend and this API behind one hostname (the reverse proxy routes `/api/*` here) and CORS stops applying at all. Cross-origin cookie auth additionally needs `COOKIE_SAME_SITE=none`, which lets the session ride cross-site requests and reintroduces CSRF as something you have to handle. With the default `SameSite=Lax`, a cross-origin frontend won't get the cookie sent at all and has to fall back to the bearer header.

---

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

| Action | Limit | Error |
|---|---|---|
| OTP send per phone | 1 per 60 seconds | `429 TOO_MANY_REQUESTS` |
| OTP send per phone | 5 per hour | `429 TOO_MANY_REQUESTS` |
| Wrong OTP guesses | 5 per challenge | `400 OTP_LOCKED` (challenge dead) |
| Code validity window | 10 minutes | `400 OTP_EXPIRED` |

There is currently **no rate limit on the password check itself** — repeated wrong-password attempts against `/login` are not throttled before the OTP step.

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
| `429` | OTP resent too soon, or too many times this hour | Show the wait; don't auto-retry |
| `502` | Upstream payment provider failed (`PAYOUT_FAILED`) | **Do not retry the same withdrawal.** The withdrawal row is already created (then marked `failed` and the balance refunded). Show the `error` string; let the user start a *new* withdrawal if they want to try again. |
| `500` | Internal error | Generic `INTERNAL_ERROR`; details stay in server logs |

### Error code catalog

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

## Pagination

`GET /transactions`, `GET /payment-requests` and `GET /withdrawals` return
`{ "data": [...], "next_cursor": "<token>" | null }`, newest first. To get the
next page, repeat the request with `?cursor=<next_cursor>` (same `limit`);
`next_cursor` is `null` on the last page. The cursor is an opaque token — don't
parse or build it. Pages are keyed on `(created_at, id)`, so rows created while
you page never shift or repeat later pages. A malformed cursor returns `400`.

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

### `POST /logout`
No auth — a browser holding an expired or malformed session still needs to clear it. Returns `204` and a `Set-Cookie` that expires `aframp_session` immediately.

Note this clears the browser's session, it does not revoke the JWT: a token already copied elsewhere stays valid until it expires. There's no server-side revocation list yet.

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

### `PATCH /me`
Auth required. Body: `{ "name"?: string, "phone_number"?: string }` (at least one). A new `name` (trimmed, 1–100 characters) applies immediately. A new `phone_number` (any common Nigerian format, normalized to `+234…`) is **not** switched yet: an OTP is sent to it and the response includes `phone_verification: { challenge_id, expires_in_secs }`. Complete it with `POST /verify-otp` (as at signup) to move the account to the new number.

`200` → `{ "name": "...", "phone_number": "<number on file>", "phone_verification": null | {...} }`. `400` with `field` for invalid input, `409` if the number belongs to another account, `429` if OTP sends to that number are rate limited.

### `DELETE /me`
Auth required. Deletes the signed-in account (right to erasure under GDPR / NDPA). Returns `204` and clears the session cookie. Every token issued for the account stops working immediately (including for `/auth/refresh`), and the email can't log in again.

**Data retention:** deletion is a soft delete. Personal data on the account is erased in place — email becomes `deleted-<id>@deleted.invalid`, name becomes "Deleted user", the phone number and password hash are cleared — and pending OTP challenges are removed. Financial records (payments, payment requests, withdrawals, wallets, balances) are kept, still linked to the anonymized account, because they're needed for the audit trail; withdrawal records keep the bank details they were paid out to for the same reason.

### `POST /wallet/create`
Auth required. Generates a **real Stellar ed25519 keypair** for the merchant. The private key is AES-256-GCM encrypted server-side and never leaves it.

```json
{ "network": "stellar" }
```
`network` is optional and defaults to `"stellar"`.

`200` →
```json
{
  "id": "18f4244e-8460-4af9-b268-28bc4b23b9ea",
  "merchant_id": "6a91d75c-8c41-4fa5-b10b-6eb8cda8ac0a",
  "address": "GDDTPSD7BWERBIKVYXJY4KMBVFCUKNGJB2CS3DWBUUO3IB2CV7BZ5WSR",
  "network": "stellar",
  "created_at": "2026-08-13T14:15:34.518727Z"
}
```

> **Calling this repeatedly creates a new wallet each time.** There's no idempotency guard. `GET /wallet` returns the most recently created one, so a duplicate call silently changes where new payment requests point. Create a wallet once during onboarding and check `GET /wallet` first.

### `GET /wallet`
Auth required. The merchant's most recent wallet. Same shape as above.

Errors: `404 "no wallet created yet"` if none exists — that's the signal to run onboarding, not an error to surface raw.

---

### `POST /payment-requests`
Auth required. **The core POS action.** Creates a request for a specific amount and returns a scannable payload.

```json
{ "amount_stroops": 25000000, "asset": "XLM", "expires_in_secs": 900 }
```

| Field | Required | Default | Notes |
|---|---|---|---|
| `amount_stroops` | yes | — | Must be a positive **integer** (`int64`). Floats (`2.5`) and strings (`"100"`) are rejected with `400` `{ code: "INVALID_PARAMETERS", field: "amount_stroops" }` — not a bare 422. |
| `asset` | no | `"XLM"` | See the cNGN caveat below |
| `expires_in_secs` | no | `900` (15 min) | Integer `int64`, clamped to 60–86400 |

`200` →
```json
{
  "id": "26b6e670-a8b1-471d-ab0f-773a9a318a6a",
  "merchant_id": "6a91d75c-8c41-4fa5-b10b-6eb8cda8ac0a",
  "address": "GDDTPSD7BWERBIKVYXJY4KMBVFCUKNGJB2CS3DWBUUO3IB2CV7BZ5WSR",
  "network": "stellar",
  "amount_stroops": 25000000,
  "asset": "XLM",
  "memo": "1f97c93409172a7d",
  "status": "pending",
  "expires_at": "2026-08-13T14:30:34.518727Z",
  "created_at": "2026-08-13T14:15:34.520195Z",
  "sep7_uri": "web+stellar:pay?destination=GDDT...&amount=2.5000000&memo=1f97c93409172a7d&memo_type=MEMO_TEXT"
}
```

**Render `sep7_uri` as the QR code.** It's a [SEP-0007](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0007.md) payment URI that Stellar wallets open natively. Generate the QR client-side (`qrcode`, `react-qr-code`) — the backend returns the string, not an image.

> **`sep7_uri` is `null` for cNGN.** There's no real cNGN issuer address configured yet, and a guessed issuer would silently misdirect a customer's money. Handle the null case — don't render a broken QR. XLM works today.

**The `memo` is what links a payment to this request.** A customer paying without it still credits the merchant's balance, but the request stays `pending` forever. The SEP-7 URI includes it automatically; if you ever show manual payment instructions, the memo is mandatory.

Errors: `404 "create a wallet before generating payment requests"` if the merchant has no wallet.
Errors: `400 "create a wallet before generating payment requests"` if the merchant has no wallet. Non-integer `amount_stroops` → `400` with `code: "INVALID_PARAMETERS"` and `field: "amount_stroops"`.

### `GET /payment-requests`
Auth required. The merchant's own requests, **newest first**. Scoped to the authenticated merchant — you cannot see another merchant's requests.

Query:
- `?limit=` (default 50, clamped 1–200)
- `?cursor=` opaque keyset cursor from a previous page
- `?include_cancelled=true` to include soft-deleted requests (default: excluded)

`200` → `{ "data": [ ... ], "next_cursor": "..." | null }`.

Cancelled requests are omitted from the default list so merchants can clean up history without losing the audit trail until the 30-day hard-delete job runs.

### `DELETE /payment-requests/{id}`
Auth required. Soft-deletes (archives) a payment request owned by the authenticated merchant by setting `cancelled_at`. Does **not** permanently remove the row.

`200` → same payment-request object with `status: "cancelled"` and `cancelled_at` set.

Errors:
- `404` if the id does not exist
- `403` if the request belongs to another merchant
- `400` if it was already cancelled

A background job hard-deletes rows that are both expired and cancelled for more than 30 days.
Query: `?limit=` (default 50, clamped 1–200) and `?cursor=` (see [Pagination](#pagination)).

`200` → `{ "data": [ …objects above… ], "next_cursor": "…" | null }`.

### `GET /payment-requests/{id}`
**No auth** — deliberately public, so a customer's device can read a request before paying.

`200` → same object. `404` if the id doesn't exist.

**Prefer `GET /payment-requests/{id}/status` for customer-side polling** (smaller payload). The full object is still useful when the wallet needs destination/memo/`sep7_uri` before paying.

| `status` | Meaning |
|---|---|
| `pending` | Not yet paid, not yet expired |
| `paid` | A memo-matched payment was detected and confirmed |
| `expired` | `expires_at` passed while still pending |
| `cancelled` | Merchant soft-deleted the request (`cancelled_at` set) |

`expired` is computed at read time, so it's accurate the moment you fetch it. A request that expires and is *then* paid still flips to `paid` — expiry doesn't block correlation. `cancelled` always wins over `expired` at read time.

### `GET /payment-requests/{id}/status`
**No auth** — lightweight public poll for customers after they submit a Stellar payment.

`200` →
```json
{ "status": "pending" }
```
or when paid:
```json
{ "status": "paid", "paid_at": "2026-08-13T14:20:01.000000Z" }
```

Uses the same `effective_status` rules as the full GET (overdue `pending` → `expired`). Response includes `Cache-Control: public, max-age=5` so browsers/CDNs can coalesce rapid polls.

Deposit detection still runs on a timer (`STELLAR_POLL_INTERVAL_SECS`, default 60s), so expect up to ~60s of latency after on-chain confirmation. Poll every 3–5s.

`404` if the id doesn't exist.

---

### `GET /balance`
Auth required. One row per asset the merchant has ever held. Returns `[]` for a new merchant — not an error.

`200` →
```json
[
  {
    "merchant_id": "6a91d75c-8c41-4fa5-b10b-6eb8cda8ac0a",
    "asset": "XLM",
    "available": 100000000000,
    "pending": 0,
    "updated_at": "2026-08-13T14:15:34.520195Z"
  }
]
```

`available` is withdrawable; `pending` is detected but not yet confirmed. In practice `pending` is almost always `0` — deposits currently move to confirmed immediately (no confirmation-depth threshold yet).

### `GET /transactions`
Auth required. Detected incoming payments, newest first. Query: `?limit=` (default 50, clamped 1–200) and `?cursor=` (see [Pagination](#pagination)). The response is a page: `{ "data": [...], "next_cursor": ... }`, with `data` items like this:

`200` →
```json
[
  {
    "id": "f03857f3-b9e3-4bb4-99df-fdab11e69143",
    "merchant_id": "6a91d75c-8c41-4fa5-b10b-6eb8cda8ac0a",
    "wallet_id": "3087fc49-6b88-4778-b22d-410fef5e9915",
    "wallet_address": "GDDTPSD7BWERBIKVYXJY4KMBVFCUKNGJB2CS3DWBUUO3IB2CV7BZ5WSR",
    "tx_hash": "4b3dc2ebaa551509e55c00240222dff650e0013236471e170512ca992e2304dc",
    "amount_stroops": 25000000,
    "asset": "XLM",
    "network": "stellar",
    "status": "confirmed",
    "confirmations": 0,
    "created_at": "2026-08-13T13:54:20.123456Z",
    "updated_at": "2026-08-13T13:54:20.987654Z"
  }
]
```

`status` is one of `detected` → `verified` → `confirmed`, or `failed`. `tx_hash` is a real Stellar hash — link it to an explorer (`https://stellar.expert/explorer/testnet/tx/{tx_hash}`).

> `confirmations` is currently always `0` — the confirmation-depth threshold isn't implemented. Don't display it as meaningful.

---

### `POST /withdraw`
Auth required. Debits the merchant's balance and initiates a Nigerian bank payout via Paystack.

```json
{ "amount_stroops": 500000000, "asset": "cNGN", "bank_code": "058", "account_number": "0123456789" }
```

| Field | Required | Notes |
|---|---|---|
| `amount_stroops` | yes | Whole multiple of `100000` (1 kobo). Must be an **integer** (`int64`) — floats/strings return `400` `{ code: "INVALID_PARAMETERS", field: "amount_stroops" }`. |
| `asset` | no | Defaults to `cNGN`; **only cNGN is accepted** |
| `bank_code` | yes | Paystack bank code, e.g. `058` GTBank, `999992` OPay |
| `account_number` | yes | Exactly 10 digits (NUBAN) |

`200` → a withdrawal object with `status`, `provider`, `provider_reference`.

Validation errors (`400`): `"insufficient available balance"`, `"withdrawals are only supported for the cNGN asset"`, `"amount_stroops must be a whole number of kobo"`, `"positive amount_stroops, bank_code, and a 10-digit account_number are required"`.

#### `502 Bad Gateway` — Paystack / payout provider failure

When the upstream payout provider (Paystack) rejects or fails the transfer, the API returns:

```json
{ "error": "<provider message>", "code": "PAYOUT_FAILED" }
```

**HTTP status:** `502 Bad Gateway`  
**Error code:** `PAYOUT_FAILED`

**What already happened server-side before this response:**

1. The merchant's available balance was debited.
2. A `withdrawals` row was inserted with `status: pending`.
3. The provider call failed.
4. The balance was **automatically refunded** and the row updated to `status: failed` with `failure_reason` set to the provider message.

**Frontend / client guidance — do NOT retry this request.**

- Do **not** auto-retry `POST /withdraw` on `502` / `PAYOUT_FAILED`. The withdrawal attempt is already persisted; a blind retry creates a second debit → provider call → refund cycle and a second failed audit row.
- Do **not** poll waiting for this attempt to flip to `completed` — it is already `failed`. Listing `GET /withdrawals` will show the failed row with `failure_reason`.
- Show the human-readable `error` string (it carries the provider's own wording).
- If the user wants to try again, they must explicitly start a **new** withdrawal (new `POST /withdraw` after fixing bank details / waiting for funding / etc.).

> **Payouts do not currently complete in production.** The Paystack integration is real, but Aframp's Paystack balance is unfunded, so live calls often return `502` with *"Your balance is not enough to fulfil this request."* That is still a `PAYOUT_FAILED` — same no-retry rules. Paystack's own minimum transfer is ₦50 = `500000000` stroops.

### `GET /withdrawals`
Auth required. Newest first. Query: `?limit=` (default 50, clamped 1–200) and `?cursor=` (see [Pagination](#pagination)).

`200` → a page, `{ "data": [...], "next_cursor": ... }`, with `data` items like:
```json
[
  {
    "id": "4d8513ce-1ba4-47d8-be93-f079f18a1c71",
    "merchant_id": "6a91d75c-8c41-4fa5-b10b-6eb8cda8ac0a",
    "amount_stroops": 500000000,
    "asset": "cNGN",
    "status": "failed",
    "provider": null,
    "provider_reference": null,
    "bank_code": "999992",
    "account_number": "****4250",
    "failure_reason": "Paystack error (HTTP 400 Bad Request): Your balance is not enough to fulfil this request",
    "created_at": "2026-08-13T17:10:50.729251Z",
    "updated_at": "2026-08-13T17:10:52.853489Z"
  }
]
```

`status` is `pending`, `processing`, `completed`, or `failed`. Show `failure_reason` on failed rows — it carries the provider's own wording.
`account_number` is always masked (`****1234` format) in API responses.

---

## Building the POS flow

The core merchant loop:

1. `POST /payment-requests` with the amount → get `id` and `sep7_uri`
2. Render `sep7_uri` as a QR code; show the amount and a countdown to `expires_at`
3. Poll `GET /payment-requests/{id}/status` every 3–5s (prefer over the full object)
4. On `status: "paid"` → show "Payment received"; on `"expired"` → offer to regenerate

```js
async function waitForPayment(id, { signal } = {}) {
  while (!signal?.aborted) {
    const res = await fetch(`${API}/payment-requests/${id}/status`, { signal });
    if (!res.ok) throw new Error(`lookup failed: ${res.status}`);
    const pr = await res.json();
    if (pr.status !== 'pending') return pr;      // 'paid' or 'expired'
    await new Promise((r) => setTimeout(r, 4000));
  }
}
```

Note step 3 needs no auth token, so a customer-facing payment page can use it directly.

---

## Admin access

Admin routes (`/admin/*`) require the `is_admin` flag on the user row. There is no self-service way to become an admin — set it directly in Postgres:

```sql
UPDATE users SET is_admin = true WHERE email = 'you@example.com';
```

The `is_admin` flag is baked into the JWT at login, so **re-login after flipping it** — outstanding tokens keep their original value for up to 24h. Then open `/admin` in a browser and sign in with that account.

### Known limitation: OTP-enabled admin accounts

The `/admin` dashboard's login form uses the old one-step `/login` flow. If the admin account has a `phone_number` set, `/login` now returns an OTP challenge instead of a session, and the dashboard has no code-entry step — it will appear to silently fail to log in.

**Options:**

1. **Keep the admin account phone-less.** An account without a `phone_number` still goes through the legacy single-step `/login` path. This is the simplest workaround today.

2. **Use `curl` / Postman to drive the two-step flow and paste the cookie manually:**

   ```bash
   # Step 1 — get the challenge id
   CHALLENGE=$(curl -sS -X POST http://127.0.0.1:3000/login \
     -H "Content-Type: application/json" \
     -d '{"email":"admin@example.com","password":"your-password"}' \
     | python3 -c "import sys,json; print(json.load(sys.stdin)['challenge_id'])")

   # Step 2 — read the OTP from cargo run output (OTP_PROVIDER=mock) or SMS, then verify
   TOKEN=$(curl -sS -X POST http://127.0.0.1:3000/verify-otp \
     -H "Content-Type: application/json" \
     -d "{\"challenge_id\":\"$CHALLENGE\",\"code\":\"YOUR_OTP_CODE\"}" \
     | python3 -c "import sys,json; print(json.load(sys.stdin)['token'])")

   echo "Token: $TOKEN"
   # Use as: curl ... -H "Authorization: Bearer $TOKEN"
   # Or in the browser: DevTools → Application → Cookies → set aframp_session to $TOKEN
   ```

3. **The dashboard now handles the OTP step inline** — if `/login` returns a `challenge_id`, a code-entry form is shown automatically (see the updated `admin_dashboard.html`).

## Not available yet

Worth knowing before you design around them:

- **No websockets / SSE.** Payment status is poll-only.
- **Sessions end after 7 days.** `POST /auth/refresh` extends a session up to 7 days from the original login; after that it's a re-login.
- **No token revocation.** `POST /logout` clears the browser's cookie; it cannot invalidate a JWT that has already been copied somewhere else.
- **No rate limiting on the password check itself.** OTP sends are throttled (60s cooldown, 5/hour per phone), but nothing yet stops repeated wrong-password guesses against `/login` before it ever gets to that step.
- **No cancel/delete on payment requests.** They can only expire naturally.
- **`DELETE /payment-requests/{id}`** soft-cancels a request (sets `cancelled_at`). CORS allows `GET`/`POST`/`DELETE`.
- **Merchant analytics** (`GET /analytics?period=7d|30d|90d`) is a proposed contract only and is not available until the endpoint is implemented. The proposed response includes daily confirmed-payment counts and volume by asset in stroops, paid requests created in the selected period, and a 0–1 paid-request success rate.
- **cNGN QR codes**, pending a real issuer address.
- **Completed payouts**, pending funding (see `PRD.md` §9.1).
