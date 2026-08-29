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

Tokens are **HS256, valid for 24 hours** either way. Claims are `sub` (user id), `merchant_id`, `iat`, `exp`.

Two things worth building for up front:

- **`merchant_id` is nullable.** `AuthResponse.merchant_id` and the JWT claim are both optional. Today signup always creates a merchant so it's always present, but the type allows `null` — an account without a merchant gets `400` from every merchant-scoped endpoint, not `401`. Don't assume non-null.
- **Expiry is silent.** There's no refresh endpoint. When a token expires, calls start returning `401` with `{"error":"invalid or expired token","code":"INVALID_CREDENTIALS"}` — treat any `401` on a previously-working call as "send the user back to login."

### CORS

Browser origins must be allowlisted server-side via the `CORS_ALLOWED_ORIGINS` env var (comma-separated, defaults to `http://localhost:3001`). Allowed methods are `GET`/`POST`; allowed headers are `Authorization` and `Content-Type`. Credentials **are** enabled, so the origin list is never mirrored back — an origin that isn't listed fails preflight.

The supported deployment is **same-origin**: serve the frontend and this API behind one hostname (the reverse proxy routes `/api/*` here) and CORS stops applying at all. Cross-origin cookie auth additionally needs `COOKIE_SAME_SITE=none`, which lets the session ride cross-site requests and reintroduces CSRF as something you have to handle. With the default `SameSite=Lax`, a cross-origin frontend won't get the cookie sent at all and has to fall back to the bearer header.

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
| `409` | Email already registered | Show on the signup form |
| `502` | Upstream payment provider failed | Transient — the `error` carries the provider's own message |
| `500` | Internal error | Generic `INTERNAL_ERROR`; details stay in server logs |

### Error code catalog

| Code | Status | When it's returned |
|---|---|---|
| `INVALID_PARAMETERS` | `400` | A required field is missing/malformed (e.g. short password, bad account_number/bank_code) |
| `INVALID_AMOUNT` | `400` | Amount is not positive, or not a whole number of kobo |
| `INSUFFICIENT_BALANCE` | `400` | Withdrawal exceeds the available balance |
| `UNSUPPORTED_ASSET` | `400` | Withdrawal asset isn't cNGN |
| `EMAIL_TAKEN` | `409` | Signup email already registered |
| `INVALID_CREDENTIALS` | `401` | Wrong password or unknown email on login |
| `USER_NOT_FOUND` | `404` | Authenticated user no longer exists |
| `MERCHANT_NOT_FOUND` | `400` | Account has no merchant (visit onboarding) |
| `WALLET_NOT_FOUND` | `400` | No wallet yet, or none created before a payment-request call |
| `PAYMENT_REQUEST_NOT_FOUND` | `404` | Payment request id doesn't exist |
| `PAYOUT_FAILED` | `502` | Upstream payment provider rejected the payout |
| `INTERNAL_ERROR` | `500` | Unexpected server error; generic message only |

---

## Conventions

**Money is always integer stroops** (`i64`), never a float. 1 unit = `10_000_000` stroops.

```js
const toDisplay = (stroops) => (stroops / 10_000_000).toFixed(7);
const toStroops = (amount) => Math.round(amount * 10_000_000);
```

Never use floating-point arithmetic to accumulate balances — convert for display only.

**Timestamps** are RFC 3339 / ISO 8601 UTC (`2026-08-13T14:15:34.520195Z`), parseable by `new Date()`.

**Ids** are UUID v4 strings.

---

## Endpoints

### `GET /health`
Liveness probe. No auth. Returns `204 No Content` with an empty body.

### `GET /`
Returns the literal string `aframp` (not JSON). Useful as a smoke test.

---

### `POST /signup`
No auth. Creates a user **and** their merchant in one transaction.

```json
{ "email": "merchant@example.com", "password": "at-least-8-chars", "name": "Shop Name" }
```

`200` →
```json
{
  "token": "eyJ0eXAiOiJKV1Qi...",
  "user_id": "2c5e0ee2-7f87-4efb-b1c9-d7e1b3ee0eeb",
  "merchant_id": "6a91d75c-8c41-4fa5-b10b-6eb8cda8ac0a"
}
```

Errors: `400` if email is empty, password is under 8 characters, or name is empty. `409` if the email is already registered.

### `POST /login`
No auth. Same response shape as signup.

```json
{ "email": "merchant@example.com", "password": "at-least-8-chars" }
```

Errors: `401` for both a wrong password and an unknown email — deliberately indistinguishable, so don't build a "no such account" message from it.

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
  "created_at": "2026-08-13T14:15:34.232320Z",
  "merchant_id": "6a91d75c-8c41-4fa5-b10b-6eb8cda8ac0a",
  "merchant_name": "Shop Name"
}
```

`merchant_id` and `merchant_name` are `null` for an account with no merchant. The password hash is never serialized.

---

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

Errors: `400 "no wallet created yet"` if none exists — that's the signal to run onboarding, not an error to surface raw.

---

### `POST /payment-requests`
Auth required. **The core POS action.** Creates a request for a specific amount and returns a scannable payload.

```json
{ "amount_stroops": 25000000, "asset": "XLM", "expires_in_secs": 900 }
```

| Field | Required | Default | Notes |
|---|---|---|---|
| `amount_stroops` | yes | — | Must be > 0 |
| `asset` | no | `"XLM"` | See the cNGN caveat below |
| `expires_in_secs` | no | `900` (15 min) | Clamped to 60–86400 |

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

Errors: `400 "create a wallet before generating payment requests"` if the merchant has no wallet.

### `GET /payment-requests`
Auth required. The merchant's own requests, **newest first**. Scoped to the authenticated merchant — you cannot see another merchant's requests.

Query: `?limit=` (default 50, clamped 1–200).

`200` → array of the object above.

> Pagination is limit-only — there's no cursor or offset, so you can't page beyond the most recent 200.

### `GET /payment-requests/{id}`
**No auth** — deliberately public, so a customer's device can read a request before paying.

`200` → same object. `404` if the id doesn't exist.

**Poll this to detect payment.** Deposit detection runs on a timer (`STELLAR_POLL_INTERVAL_SECS`, default 60s), so a payment typically shows up within ~60s of confirming on-chain, not instantly. Poll every 3–5s and show a "waiting for payment" state; don't expect a sub-second flip.

| `status` | Meaning |
|---|---|
| `pending` | Not yet paid, not yet expired |
| `paid` | A memo-matched payment was detected and confirmed |
| `expired` | `expires_at` passed while still pending |

`expired` is computed at read time, so it's accurate the moment you fetch it. A request that expires and is *then* paid still flips to `paid` — expiry doesn't block correlation.

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
Auth required. Detected incoming payments, newest first. Query: `?limit=` (default 50, clamped 1–200).

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
| `amount_stroops` | yes | Must be a whole multiple of `100000` (1 kobo) |
| `asset` | no | Defaults to `cNGN`; **only cNGN is accepted** |
| `bank_code` | yes | Paystack bank code, e.g. `058` GTBank, `999992` OPay |
| `account_number` | yes | Exactly 10 digits (NUBAN) |

`200` → a withdrawal object with `status`, `provider`, `provider_reference`.

Validation errors (`400`): `"insufficient available balance"`, `"withdrawals are only supported for the cNGN asset"`, `"amount_stroops must be a whole number of kobo"`, `"positive amount_stroops, bank_code, and a 10-digit account_number are required"`.

> **Payouts do not currently complete.** The Paystack integration is real and correct, but Aframp's Paystack balance is unfunded, so live calls return `502` with *"Your balance is not enough to fulfil this request."* On failure the balance is **automatically refunded** and the withdrawal is recorded with `status: "failed"` and a `failure_reason` — no money or ledger record is lost. Treat `502` as "try later," not as data loss. Paystack's own minimum transfer is ₦50 = `500000000` stroops.

### `GET /withdrawals`
Auth required. Newest first. Query: `?limit=` (default 50, clamped 1–200).

`200` →
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
    "account_number": "8038714250",
    "failure_reason": "Paystack error (HTTP 400 Bad Request): Your balance is not enough to fulfil this request",
    "created_at": "2026-08-13T17:10:50.729251Z",
    "updated_at": "2026-08-13T17:10:52.853489Z"
  }
]
```

`status` is `pending`, `processing`, `completed`, or `failed`. Show `failure_reason` on failed rows — it carries the provider's own wording.

---

## Building the POS flow

The core merchant loop:

1. `POST /payment-requests` with the amount → get `id` and `sep7_uri`
2. Render `sep7_uri` as a QR code; show the amount and a countdown to `expires_at`
3. Poll `GET /payment-requests/{id}` every 3–5s
4. On `status: "paid"` → show "Payment received"; on `"expired"` → offer to regenerate

```js
async function waitForPayment(id, { signal } = {}) {
  while (!signal?.aborted) {
    const res = await fetch(`${API}/payment-requests/${id}`, { signal });
    if (!res.ok) throw new Error(`lookup failed: ${res.status}`);
    const pr = await res.json();
    if (pr.status !== 'pending') return pr;      // 'paid' or 'expired'
    await new Promise((r) => setTimeout(r, 4000));
  }
}
```

Note step 3 needs no auth token, so a customer-facing payment page can use it directly.

---

## Not available yet

Worth knowing before you design around them:

- **No websockets / SSE.** Payment status is poll-only.
- **No refresh tokens.** A 24h expiry means a re-login, not a silent refresh.
- **No token revocation.** `POST /logout` clears the browser's cookie; it cannot invalidate a JWT that has already been copied somewhere else.
- **No rate limiting on `/login`.** Nothing throttles password guessing yet.
- **No cursor pagination.** `limit` only, capped at 200.
- **No cancel/delete on payment requests.** They can only expire naturally.
- **No `PATCH`/`DELETE` anywhere** — and CORS only allows `GET`/`POST`, so adding one needs a server change too.
- **cNGN QR codes**, pending a real issuer address.
- **Completed payouts**, pending funding (see `PRD.md` §9.1).
