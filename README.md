# Aframp

**Building the POS network for Stellar in Africa.**

Aframp brings Stellar-powered payments into everyday physical commerce, starting in Nigeria. The idea is simple: Nigerians already understand the POS terminal — tap, transfer, withdraw. Aframp adds another familiar option on top of that muscle memory: **scan and pay**, settled on Stellar, without the merchant or customer ever needing to think about wallets, addresses, or blockchains.

```
Customer → Scan → Pay → Confirmed
```

From the merchant's point of view, that's the whole product. Stellar is the settlement layer underneath; Aframp is the experience on top.

## Why

Cross-border commerce in Africa is fragmented across national payment systems — multiple currencies, high fees, slow settlement, and merchant onboarding that doesn't travel across borders. Stablecoins and blockchain rails already move value globally, fast and cheap. What's missing is the everyday, physical-commerce bridge between the two. Aframp aims to be that bridge: onboard merchants first, and let consumer demand for Stellar wallets follow.

Longer-term shape of the platform (not all built yet — see [Status](#status-real-progress-not-aspiration) below):

| Layer | Purpose |
|---|---|
| **Aframp Pay** | Merchant-facing payments: requests, QR codes, receive Stellar payments, receipts, revenue tracking |
| **Aframp Wallet** | Consumer wallet built for spending, not just holding |
| **Aframp Business** | Merchant dashboard: analytics, reconciliation, multi-location, invoices |
| **Aframp API** | Infrastructure layer for other African fintechs to integrate Stellar payments |

This repository is the backend underneath **Aframp Pay**.

## Status: real progress, not aspiration

This section is deliberately literal: everything marked ✅ has been exercised end-to-end against a real Postgres database and, where it touches Stellar, a real testnet transaction — not asserted from reading the code. Everything marked 🚧 is a genuine gap, described honestly rather than smoothed over.

### What's real today

| Capability | Status | Proof |
|---|---|---|
| Merchant signup / login | ✅ Done | Argon2-hashed passwords, real Postgres rows, real HMAC-signed JWTs |
| Every money/data endpoint requires auth | ✅ Done | `/wallet`, `/balance`, `/transactions`, `/withdraw*` all return `401` with no token or a garbage token — enforced at the type level via Axum's `AuthUser` extractor, not an ad hoc check |
| Stellar wallet generation | ✅ Done | Each `/wallet/create` call generates a **real ed25519 keypair**, encoded as a genuine Stellar `G...` address — not a placeholder string. Verified against Horizon directly. |
| Wallet key custody | ✅ Done | The private key (`S...` seed) is AES-256-GCM encrypted before it ever touches the database, keyed by `WALLET_ENCRYPTION_KEY`. Confirmed the API response never includes it, and the DB column holds ciphertext, not plaintext. |
| Stellar deposit detection | ✅ Done | Background worker polls Horizon's payments feed per merchant wallet, on a timer, handling both `create_account` (how a brand-new wallet is always first funded on Stellar) and regular `payment` ops. Proven with a real testnet transaction: funded a generated wallet via Stellar's friendbot, and watched the exact transaction hash and amount land in `/transactions` and `/balance` within one poll cycle. |
| Balance ledger | ✅ Done | `/balance` reflects real detected deposits, not a stub — because the above is real, this is real too. |
| Merchant transaction history | ✅ Done | `/transactions` lists real detected payments |
| Payment request generation | ✅ Done | `POST /payment-requests` creates a request with a unique correlation memo and expiry; `GET /payment-requests/{id}` is deliberately public (no auth) so a customer's wallet can read it before paying |
| QR-based payment (XLM) | ✅ Done | Every XLM payment request includes a SEP-0007 `web+stellar:pay` URI. Verified live: correct destination address, exact amount conversion (`25000000` stroops → `2.5000000` in the URI). Detection now correlates a specific incoming payment to its request via Stellar memo (Horizon queried with `join=transactions`), not just "something arrived" |
| Withdrawal request + ledger accounting | ✅ Done | `/withdraw` atomically debits `available` balance and records the request; insufficient-balance and validation checks are enforced in the same DB transaction |
| Paystack Transfers wired into `/withdraw` | ✅ Done | Real calls to Paystack's Transfers API (resolve account → create recipient → initiate transfer), live-tested against three distinct real failure modes (bad account, amount below minimum, insufficient platform balance) — every one correctly triggers a refund + `failed` audit-trail row, never a silent loss of the ledger record |

### What's still a stub or missing

| Gap | What's actually there today |
|---|---|
| QR-based payment (cNGN) | `sep7_uri` is `null` for cNGN payment requests — there's no real cNGN issuer Stellar address configured, and a guessed one would silently misdirect a customer's payment. Works for XLM today; cNGN needs a real issuer address sourced first |
| Real payout funding ("Stage A") | Paystack Transfers are wired and code-correct (see above), but Paystack's own business-account balance is ₦0 — `source: "balance"` transfers have nothing to draw from. Confirmed live: a real bank account + valid amount still failed with *"Your balance is not enough to fulfil this request."* Nothing pays out until there's a real crypto→fiat funding pipeline (e.g. cNGN issuer redemption) |
| Confirmation-depth threshold | Deposits move `detected → verified → confirmed` immediately on detection — there's no real "wait N ledger confirmations" logic yet (Stellar has fast finality, so this matters less than on Bitcoin, but it's still an open TODO in `blockchain/worker.rs`) |
| Settlement/sweep wallet | Each merchant's Stellar secret is held (encrypted) by the platform, but nothing yet sweeps funds from individual merchant wallets into a platform settlement wallet. `STELLAR_SYSTEM_WALLET_ADDRESS` is still validated at startup and reserved for this, but isn't used by anything yet |
| TLS | The server speaks plain HTTP by design and must run behind a TLS-terminating reverse proxy. Deployed without one, passwords cross the network in cleartext and no amount of hashing helps — the attacker sees the password before it is hashed. See [Deploying behind TLS](#deploying-behind-tls) |
| Login rate limiting | Nothing throttles password guessing against `/login` yet |
| Token revocation | `POST /logout` clears the browser cookie, but a JWT already copied elsewhere stays valid for its full 24h. No revocation list, no refresh rotation |
| `src/stellar/mod.rs` | Vestigial stub from an earlier, abandoned design (single system wallet + memo-based correlation). Not compiled into the binary's active module tree in any meaningful way, superseded by the per-wallet design in `src/blockchain/`. Left in place as known cleanup debt rather than silently deleted. |

See **[`PRD.md`](PRD.md)** for the full open-decisions list (payout provider choice, cNGN issuer sourcing, confirmation policy) and roadmap.

## How it actually works today

- A merchant signs up (`/signup`) and creates a wallet (`/wallet/create`), which generates a real Stellar keypair. The public address is returned; the private key is encrypted and stored server-side — this is a **custodial** design, not "bring your own wallet."
- A merchant creates a payment request for an amount (`/payment-requests`) and gets back a destination, a correlation memo, and — for XLM — a scannable QR payload.
- A background worker polls Horizon for every merchant wallet's address on a timer (`STELLAR_POLL_INTERVAL_SECS`), detects incoming payments (with the transaction memo joined in), and moves them through Postgres into that merchant's balance — marking the matching payment request `paid` if the memo correlates to one.
- Merchants can withdraw their available balance (`/withdraw`) to a Nigerian bank account. The Paystack integration itself is real and tested; what's missing is money to actually send — see gaps above.

The originally-planned memo-based correlation model (one shared system wallet, deposits routed by transaction memo) was replaced with real per-merchant wallets during development — simpler to reason about and matches how a QR-code-per-merchant product actually needs to work.

## Tech stack

- **Rust** + [Axum](https://github.com/tokio-rs/axum) — HTTP API
- **PostgreSQL** via [sqlx](https://github.com/launchbadge/sqlx) — runtime-checked queries
- **Stellar** ([Horizon](https://developers.stellar.org/docs/data/horizon)) — settlement network; deposit detection via [reqwest](https://github.com/seanmonstar/reqwest)
- **ed25519-dalek** + **stellar-strkey** — real Stellar keypair generation
- **aes-gcm** — encrypts wallet private keys at rest
- **JWT** ([jsonwebtoken](https://github.com/Keats/jsonwebtoken)) + **Argon2** — auth and password hashing
- **Tokio** — async runtime, including the background Stellar polling worker
- **Paystack Transfers API** — Nigerian bank payouts (`src/payments/paystack.rs`)

## Getting started

### Prerequisites

- Rust (stable, 2021 edition)
- PostgreSQL — either a local instance, or Docker (see below; this is the path actually verified during development on a machine where the local Postgres cluster wasn't running)

### Setup

```bash
cp .env.example .env
```

Fill in `.env`:

| Variable | Required | Default | Description |
|---|---|---|---|
| `DATABASE_URL` | yes | — | Postgres connection string |
| `APP_BIND_ADDR` | no | `127.0.0.1:3000` | Address the HTTP server binds to |
| `JWT_SECRET` | yes | — | Secret used to sign merchant session tokens. Generate with `openssl rand -hex 32` |
| `WEBHOOK_SECRET` | yes | — | Secret used to verify inbound provider webhooks. Generate with `openssl rand -hex 32` |
| `WALLET_ENCRYPTION_KEY` | yes | — | AES-256-GCM key encrypting Stellar wallet secrets at rest. Generate with `openssl rand -hex 32` (must decode to exactly 32 bytes) |
| `WALLET_ENCRYPTION_KEY_NEW` | only for `--rotate-key` | — | The key to rotate `WALLET_ENCRYPTION_KEY` to. See "Rotating `WALLET_ENCRYPTION_KEY`" below |
| `STELLAR_SYSTEM_WALLET_ADDRESS` | yes | — | Reserved for a future platform settlement/sweep wallet. Validated at startup but not used by deposit detection today (see [Status](#status-real-progress-not-aspiration)) |
| `STELLAR_HORIZON_URL` | no | `https://horizon-testnet.stellar.org` | Horizon endpoint to poll |
| `STELLAR_POLL_INTERVAL_SECS` | no | `60` | How often the deposit-detection worker polls Horizon, per wallet |
| `PAYSTACK_SECRET_KEY` | yes | — | Paystack Dashboard → Settings → API Keys & Webhooks. `sk_test_...` for dev, `sk_live_...` only once the business is verified/activated for Transfers (see `PRD.md` §9.1) |
| `CORS_ALLOWED_ORIGINS` | no | `http://localhost:3001` | Comma-separated browser origins allowed to call the API. Never mirrored back — an unlisted origin fails preflight |
| `COOKIE_SECURE` | no | `true` | Whether the session cookie carries `Secure`. Leave on: browsers treat `localhost` as a secure context, so the default works in dev too. Only turn it off for a non-localhost plain-HTTP setup, which you should not have |
| `COOKIE_SAME_SITE` | no | `lax` | `lax` or `none`. `none` (which forces `Secure`) is only for a frontend on a different origin, and lets the session ride cross-site requests — prefer serving the frontend same-origin |

### Rotating `WALLET_ENCRYPTION_KEY`

```bash
# 1. Generate the new key
openssl rand -hex 32

# 2. Re-encrypt every wallet's secret from the old key to the new one
WALLET_ENCRYPTION_KEY=<current-key> \
WALLET_ENCRYPTION_KEY_NEW=<new-key> \
DATABASE_URL=<target-database> \
  cargo run -- --rotate-key

# 3. On success, update the deployment's WALLET_ENCRYPTION_KEY to <new-key>
#    and restart. Discard the old key.
```

No downtime is required for this today: nothing in the running server decrypts a
wallet secret (withdrawals settle through Paystack, not by signing with the
wallet's own key), so there is no live reader for the rotation to race
against. See the doc comment on `src/rotate_key.rs` for what would need to
change once that's no longer true — e.g. once a sweep-wallet signer exists.

### Quick start (Docker Postgres)

```bash
docker run -d --name aframp-postgres \
  -e POSTGRES_USER=postgres -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=aframp \
  -p 5432:5432 postgres:16

for f in migrations/*.sql; do
  docker exec -i aframp-postgres psql -U postgres -d aframp < "$f"
done

cargo run
```

### Alternative: sqlx-cli against an existing Postgres

```bash
cargo install sqlx-cli --no-default-features --features rustls,postgres
sqlx database create
sqlx migrate run

cargo run
```

The server starts on `APP_BIND_ADDR` (default `http://127.0.0.1:3000`) and spawns the Stellar deposit-detection worker in the background.

See **[`command.txt`](command.txt)** for a copy-paste reference of every command used to run, test, and interact with this backend — curl calls for every endpoint, secret generation, and DB lifecycle commands.

### Cloudflare Containers

This repository includes a Cloudflare Container deployment scaffold. It runs the existing Axum binary in a single named container, with the `aframp-backend` Worker acting as the public HTTPS proxy. The single instance is intentional: multiple instances would each run the Stellar polling loop and duplicate work.

Before deploying:

1. In [`wrangler.jsonc`](wrangler.jsonc), replace `CORS_ALLOWED_ORIGINS` with the exact HTTPS origin of the frontend. If the frontend is on another origin, set `COOKIE_SAME_SITE` to `none`; it requires the existing `COOKIE_SECURE=true` setting.
2. Store each required value as a Cloudflare Worker secret. Do not copy `.env` into source control or `wrangler.jsonc`:

   ```bash
   npx wrangler secret put DATABASE_URL
   npx wrangler secret put JWT_SECRET
   npx wrangler secret put WEBHOOK_SECRET
   npx wrangler secret put WALLET_ENCRYPTION_KEY
   npx wrangler secret put STELLAR_SYSTEM_WALLET_ADDRESS
   npx wrangler secret put PAYSTACK_SECRET_KEY
   ```

3. Run the SQL migrations against the production database, then deploy:

   ```bash
   cargo install sqlx-cli --no-default-features --features rustls,postgres
   sqlx migrate run
   npm install
   npx wrangler deploy
   ```

The Worker has a five-minute Cron Trigger that calls `/health`. This keeps the container awake so its 60-second Stellar polling loop continues when there is no API traffic. Cloudflare may take several minutes to provision the first container after deployment.

### Running tests

Integration tests need a separate database, and **silently skip with a false "ok" if it isn't configured** — this bit us during development (a full green `cargo test` run had actually tested nothing). Always set `TEST_DATABASE_URL` before trusting the result:

```bash
docker exec -i aframp-postgres psql -U postgres -c "CREATE DATABASE aframp_test;"
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/aframp_test cargo test
```

## API reference

**Building a frontend?** Start with **[`API.md`](API.md)** — the full contract with request/response examples, error semantics, the stroops convention, polling guidance, and the known gaps worth designing around. **[`openapi.yaml`](openapi.yaml)** is the machine-readable version; generate a typed client from it rather than hand-writing calls.

The table below is a quick index.

Authenticated routes accept either the `aframp_session` HttpOnly cookie (set by `/signup` and `/login` — what browsers should use) or `Authorization: Bearer <token>` with the JWT from the same responses (for scripts and API clients). See [`API.md`](API.md#authentication).

| Method | Path | Auth | Description |
|---|---|---|---|
| `POST` | `/signup` | — | Create a user + merchant account. Body: `{ email, password (min 8 chars), name }` |
| `POST` | `/login` | — | Authenticate. Body: `{ email, password }` |
| `POST` | `/logout` | — | Expire the session cookie. Ends the browser session; does not revoke the JWT |
| `GET` | `/me` | ✅ | Current user + merchant profile. The JWT carries only ids, so a reloaded frontend needs this to render identity |
| `POST` | `/wallet/create` | ✅ | Generate a real Stellar wallet for the authenticated merchant. Body: `{ network? }` (defaults to `stellar`) |
| `GET` | `/wallet` | ✅ | Get the merchant's wallet |
| `GET` | `/balance` | ✅ | List balances by asset, reflecting real detected Stellar deposits |
| `GET` | `/transactions?limit=` | ✅ | List the merchant's detected payments (default limit 50, max 200) |
| `POST` | `/payment-requests` | ✅ | Create a payment request for the authenticated merchant's wallet. Body: `{ amount_stroops, asset? (default XLM), expires_in_secs? (60–86400, default 900) }` |
| `GET` | `/payment-requests?limit=` | ✅ | List the merchant's own requests, newest first (default 50, max 200) |
| `GET` | `/payment-requests/{id}` | — | Deliberately public — a customer's wallet needs to read amount/destination/status before paying. Includes `sep7_uri` for XLM requests (`null` for cNGN — no issuer address configured yet) |
| `POST` | `/withdraw` | ✅ | Debit available balance, record a withdrawal, and call Paystack Transfers. Body: `{ amount_stroops, asset? (cNGN only), bank_code, account_number }`. **Note:** the Paystack call is real, but nothing actually pays out yet — Paystack's own account balance is unfunded (Stage A gap) — see [Status](#status-real-progress-not-aspiration) |
| `GET` | `/withdrawals?limit=` | ✅ | List the merchant's withdrawals, including `failure_reason` on failed ones |
| `GET` | `/health` | — | Liveness check (`204 No Content`) |

`/signup` and `/login` both return:

```json
{ "token": "...", "user_id": "...", "merchant_id": "..." }
```

…alongside a `Set-Cookie: aframp_session=<jwt>; HttpOnly; Path=/; SameSite=Lax; Max-Age=86400; Secure`. A browser frontend should use the cookie and ignore the `token` field — copying it into `localStorage` puts the session within reach of any XSS on the page.

## Deploying behind TLS

The server intentionally does not terminate TLS. It binds to `127.0.0.1:3000` by default and expects a reverse proxy in front of it — which is also what makes the `Secure` session cookie meaningful, since browsers won't send a `Secure` cookie over plain HTTP to a non-localhost host.

Serve the frontend and the API from **one origin**. Then browser requests are same-origin, CORS stops applying entirely, and the `SameSite=Lax` cookie works without the cross-site relaxation that reintroduces CSRF. A minimal Caddy config:

```caddyfile
pay.example.com {
    encode gzip
    header Strict-Transport-Security "max-age=31536000; includeSubDomains"

    handle /api/* {
        uri strip_prefix /api
        reverse_proxy 127.0.0.1:3000
    }
    handle {
        root * /srv/aframp-frontend
        try_files {path} /index.html
        file_server
    }
}
```

Caddy provisions and renews the certificate itself. With this, the frontend calls `/api/me` as a relative path, sends no `Authorization` header, and never touches the token.

If the frontend genuinely must live on a separate origin, set `CORS_ALLOWED_ORIGINS` to it and `COOKIE_SAME_SITE=none` — and understand that you have then opted into cross-site cookie sending and need to think about CSRF. (Today's saving grace is that every mutating route uses the JSON extractor, so a cross-site HTML form POST can't reach one; don't rely on that if you add form-encoded endpoints.)

## Project layout

```
src/
  api/         HTTP handlers (thin — validation + calling services)
  auth/        JWT signing/verification, password hashing, auth extractor
  blockchain/  Stellar integration: keypair generation, wallet-secret encryption,
               Horizon deposit polling, and the background worker that drives it
  models/      Request/response and row types
  services/    Business logic (users, wallets, balances, payments, payment_requests, withdrawals)
  payments/    PaymentProvider abstraction — real PaystackProvider + a MockProvider for tests
  stellar/     Vestigial unused stub from an earlier design — see Status
migrations/    SQL schema migrations (sqlx)
tests/         Integration tests (auth, wallet, payment request, withdrawal flows)
examples/      prove_payment_loop.rs — end-to-end demo harness (real testnet payment)
API.md         Full API contract for frontend consumers
openapi.yaml   Machine-readable spec (generate a typed client from this)
command.txt    Copy-paste command reference for running/testing/interacting with the backend
```

## Why Nigeria first

Nigeria has a large digital-payments ecosystem and near-universal familiarity with POS and bank-transfer payments — the exact behavior Aframp is extending rather than replacing. The plan is to prove the merchant payment experience narrowly here, then expand to other African markets and cross-border corridors.

## Contributing

This project is under active MVP development — expect the API and schema to change as real payout funding, a real cNGN issuer address, and settlement sweeping land. Open an issue or PR against `master`.
