# Changelog

All notable changes to Aframp Backend are documented here.

This file follows the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
format. Aframp Backend does not yet publish versioned releases — changes are
tracked by feature area and date instead of semver tags. When versioned
releases begin, this file will adopt the standard `[x.y.z] - YYYY-MM-DD`
headings.

> **Maintainer note:** Every PR that touches the public API contract
> (`src/api/`, `src/models/`, `API.md`, `openapi.yaml`) must add an entry to
> the relevant `[Unreleased]` section before merging.

---

## [Unreleased]

### Added
- `POST /auth/refresh` — swap a still-valid JWT for a new 24-hour token, for
  up to 7 days from original login. See `API.md § Authentication`.
- `/admin/overview`, `/admin/merchants`, `/admin/users`, `/admin/wallets`,
  `/admin/transactions`, `/admin/withdrawals`, `/admin/payment-requests` —
  system-wide admin list views (requires `is_admin = true` on the user row).
- `GET /admin` — static admin dashboard shell (HTML).
- `GET /payment-requests/{id}/status` — lightweight public poll endpoint;
  returns `{ status, paid_at? }` with `Cache-Control: public, max-age=5`.
  Prefer this once a customer has submitted payment instead of polling the
  full `/payment-requests/{id}`.
- Cloudflare Containers deployment scaffold (`wrangler.jsonc`, `cloudflare/`).
  A 5-minute Cron Trigger keeps the container awake for the Stellar poll loop.
- Architecture Decision Records in `docs/adr/`:
  - ADR-001: Per-merchant custodial wallets
  - ADR-002: OTP-gated signup / full 2FA on login
  - ADR-003: HMAC-SHA256 for OTP storage (not Argon2)
  - ADR-004: Commit withdrawal before calling Paystack; compensate on failure
- Exponential backoff for unfunded Stellar wallets — wallets that have never
  been funded (Horizon 404) are polled less frequently (30 s base, doubles per
  consecutive 404, capped at 10 minutes) rather than on every cycle.
- `claimable_balance_created` operation type is now detected by the deposit
  worker in addition to `payment`, `create_account`, and
  `path_payment_strict_*`.

### Changed
- Deposit correlation: the worker now matches incoming payments to payment
  requests via Stellar transaction memo (Horizon queried with
  `join=transactions`) instead of amount alone.

---

## OTP Rollout — Breaking change (2024, exact date TBD)

This was the largest breaking change to the authentication flow. **All
frontend and API clients** that relied on the old one-step login must be
updated.

### What changed

#### Before OTP

`POST /signup` and `POST /login` returned a session directly:

```json
{ "token": "...", "user_id": "...", "merchant_id": "..." }
```

`Set-Cookie: aframp_session=<jwt>` was set in the same response.

#### After OTP (current behaviour)

`POST /signup` and `POST /login` **no longer return a session**. They return
a challenge instead:

```json
{ "challenge_id": "...", "expires_in_secs": 600 }
```

The session is now issued **only** by `POST /verify-otp`:

```
POST /verify-otp
{ "challenge_id": "<from signup or login>", "code": "<6-digit SMS code>" }
```

On success, `/verify-otp` returns the same shape as the old login:

```json
{ "token": "...", "user_id": "...", "merchant_id": "..." }
```

…alongside `Set-Cookie: aframp_session=<jwt>`.

### Migration path for existing clients

| Old call | New call sequence |
|----------|-------------------|
| `POST /signup` → get token | `POST /signup` → get `challenge_id`, then `POST /verify-otp` → get token |
| `POST /login` → get token | `POST /login` → get `challenge_id`, then `POST /verify-otp` → get token |

**Step-by-step for a login flow:**

1. `POST /login` with `{ email, password }`.
2. Parse `challenge_id` from the response.
3. Display a 6-digit code entry screen to the user.
4. `POST /verify-otp` with `{ challenge_id, code }`.
5. Parse `token` / read the session cookie from the response — this is your
   session.

**Resend OTP:** re-`POST` to `/signup` or `/login` with the same credentials.
There is no separate `/resend-otp` endpoint. Rate limits: 1 send per 60 s per
phone, 5 per hour per phone.

### Admin accounts and OTP

Admin accounts with a `phone_number` on the row now receive the two-step
challenge on `/login`. The `/admin` dashboard HTML only knows the old one-step
flow and will appear to fail to log in for phone-bearing admin accounts. Until
the dashboard is updated:

- Keep admin accounts phone-less (no `phone_number` in the `users` row), or
- Drive the flow manually: `POST /login` → `POST /verify-otp` → copy the
  resulting cookie into the browser by hand (DevTools → Application →
  Cookies).

---

## Per-merchant wallet architecture — Breaking change (pre-OTP)

### What changed

The original design used a single shared Stellar system wallet with per-payment
memos to route deposits. This was replaced with one Stellar keypair per
merchant.

- `POST /wallet/create` generates a real ed25519 keypair for the authenticated
  merchant. The public address is returned; the private key is AES-256-GCM
  encrypted and stored server-side.
- `GET /wallet` returns the merchant's wallet (public address only — the
  private key is never exposed via the API).
- `STELLAR_SYSTEM_WALLET_ADDRESS` is still validated at startup and reserved
  for a future settlement/sweep wallet, but is no longer used for deposit
  routing.

### Impact

If you previously relied on memo-based routing to a shared wallet, that
integration no longer works. Each merchant now has an independent deposit
address returned by `GET /wallet`.

---

## Payment requests — Added

- `POST /payment-requests` — create a payment request with a unique
  correlation memo and expiry.
- `GET /payment-requests/{id}` — **deliberately public** (no auth), so a
  customer's wallet can read amount/destination/status before paying.
  Includes `sep7_uri` for XLM requests (`null` for cNGN — no real issuer
  address configured yet).
- `GET /payment-requests?limit=` — list the authenticated merchant's own
  requests.
- `sep7_uri` uses the `web+stellar:pay` SEP-0007 URI scheme. QR code
  generation from this URI is left to the client.

---

## Withdrawal flow — Added

- `POST /withdraw` — atomically debits `available` balance and records the
  withdrawal; calls Paystack Transfers API for Nigerian bank payout.
  Insufficient-balance and validation errors are enforced within the same
  database transaction.
- `GET /withdrawals?limit=` — list the merchant's withdrawals, including
  `failure_reason` on failed entries.
- On Paystack failure (bad account, below minimum, insufficient platform
  balance), the withdrawal is marked `failed` and the balance is refunded
  atomically — the ledger record is never silently lost.

---

## Known gaps (not yet in the changelog but tracked in README.md / PRD.md)

| Gap | Status |
|-----|--------|
| Real payout funding (Stage A) | Paystack wired but Paystack account balance is ₦0 |
| cNGN payment requests | `sep7_uri` is `null` — no issuer address configured |
| Confirmation-depth threshold | Deposits go `detected → confirmed` immediately |
| Settlement/sweep wallet | `STELLAR_SYSTEM_WALLET_ADDRESS` reserved but unused |
| Login rate limiting | No throttle on `/login` |
| Token revocation | JWTs stay valid 24 h after `/logout` |
| Admin dashboard OTP | Dashboard broken for phone-bearing admin accounts |
