# Deploying Aframp Backend

This is the production deployment guide. For local development, see the
[README](README.md#getting-started); for contributing, see
[CONTRIBUTING.md](CONTRIBUTING.md).

There are two supported shapes:

1. **[Cloudflare Containers](#1-cloudflare-containers)** — the scaffold in this
   repo (`wrangler.jsonc` + `cloudflare/worker.ts`). The Worker is the public
   HTTPS edge and proxies every request to a single container running the Axum
   binary built from the `Dockerfile`.
2. **[A VM behind Caddy](#2-a-vm-behind-caddy)** — the binary (or the Docker
   image) on a host you control, with Caddy terminating TLS.

Whichever you choose, read [the single-instance constraint](#the-single-instance-constraint)
first — it applies to both.

---

## The single-instance constraint

**Run exactly one instance of the backend.** Each process spawns the Stellar
deposit-detection worker (`src/blockchain/worker.rs`), which polls Horizon every
`STELLAR_POLL_INTERVAL_SECS` for every merchant wallet. Two instances means two
independent polling loops racing over the same wallets: duplicated Horizon
traffic (and rate-limit pressure), and two writers competing to record and
promote the same deposits. Other state is process-local too — the login rate
limiter in `src/api/auth.rs` and the unfunded-wallet backoff map in
`src/blockchain/stellar.rs` live in memory, so they silently stop working as
intended across instances.

That's why `wrangler.jsonc` pins `max_instances: 1` and `cloudflare/worker.ts`
routes every request to one named Durable Object (`getByName("primary")`).
Scaling out requires first moving Stellar polling to a singleton job and the
rate limiter to a shared store (e.g. Redis) — don't raise `max_instances`
before that.

---

## 1. Cloudflare Containers

### Prerequisites

- A Cloudflare account with **Containers** enabled (Workers Paid plan).
- Node.js 18+ and the repo's npm dependencies: `npm install` (installs
  `wrangler` and `@cloudflare/containers`).
- Docker running locally — `wrangler deploy` builds the image from `Dockerfile`.
- A production **PostgreSQL** database reachable from Cloudflare (e.g. Neon,
  Supabase, RDS). Size `DB_MAX_CONNECTIONS` to its connection limit.
- `sqlx-cli` to run migrations:
  `cargo install sqlx-cli --no-default-features --features rustls,postgres`
- A Stellar system wallet address (`STELLAR_SYSTEM_WALLET_ADDRESS`), a Paystack
  secret key, and — when `OTP_PROVIDER=termii` — Termii credentials.

Authenticate once with `npx wrangler login`.

### Step 1 — configure plain-text vars

Edit the `vars` block in [`wrangler.jsonc`](wrangler.jsonc) (each entry is
commented there). At minimum, replace `CORS_ALLOWED_ORIGINS` with the exact
HTTPS origin of your frontend. If the frontend is on a different origin from
the API, set `COOKIE_SAME_SITE` to `none` (requires `COOKIE_SECURE=true`, which
is already set). Switch `STELLAR_HORIZON_URL` to `https://horizon.stellar.org`
for mainnet.

### Step 2 — set secrets

Never put secrets in `wrangler.jsonc` or commit a `.env`. Store each one as a
Worker secret:

```bash
npx wrangler secret put DATABASE_URL
npx wrangler secret put JWT_SECRET               # >= 32 bytes: openssl rand -hex 32
npx wrangler secret put WEBHOOK_SECRET           # >= 32 bytes
npx wrangler secret put WALLET_ENCRYPTION_KEY    # >= 32 bytes; losing it loses every wallet seed
npx wrangler secret put OTP_HMAC_SECRET          # >= 32 bytes
npx wrangler secret put STELLAR_SYSTEM_WALLET_ADDRESS
npx wrangler secret put PAYSTACK_SECRET_KEY
# When OTP_PROVIDER=termii (the default):
npx wrangler secret put TERMII_API_KEY
npx wrangler secret put TERMII_SENDER_ID
npx wrangler secret put TERMII_WEBHOOK_SECRET
```

> **Known gap — check before deploying.** A Worker secret only reaches the
> Rust process if `cloudflare/worker.ts` forwards it in `AframpContainer.envVars`.
> Today that list forwards `DATABASE_URL`, `JWT_SECRET`, `WEBHOOK_SECRET`,
> `WALLET_ENCRYPTION_KEY`, `STELLAR_SYSTEM_WALLET_ADDRESS`, `PAYSTACK_SECRET_KEY`
> and the plain-text vars — but **not** `OTP_HMAC_SECRET`, `OTP_PROVIDER`,
> `TERMII_API_KEY`, `TERMII_SENDER_ID` or `TERMII_WEBHOOK_SECRET`. Because
> `OTP_HMAC_SECRET` is required at startup, the container exits with
> `OTP_HMAC_SECRET is required` until those entries (and their `Env` fields)
> are added to `cloudflare/worker.ts`. `cloudflare/` is a maintainer-only path;
> see [CONTRIBUTING.md](CONTRIBUTING.md#protected-paths-do-not-modify).

### Step 3 — migrate, then deploy

The container does **not** run migrations. Apply them first, against the
production database:

```bash
DATABASE_URL='postgres://...production...' sqlx migrate run
npx wrangler deploy
```

`wrangler deploy` builds the image, pushes it, and deploys the Worker. The
first container can take several minutes to provision.

### Health-check keepalive (cron trigger)

The container sleeps after `sleepAfter = "10m"` without traffic, and a sleeping
container doesn't poll Stellar. So `wrangler.jsonc` declares a cron trigger
(`*/5 * * * *`), and the Worker's `scheduled()` handler fetches
`http://container.internal/health` every five minutes, waking the container if
needed. `GET /health` returns `200` with `{"status":"ok","version":"..."}`; the
handler throws on any other status so the cron run is marked failed in the
Cloudflare dashboard rather than silently "succeeding".

If you rename `/health`, update `scripts/health_path.ts`, `cloudflare/worker.ts`
and the Rust route together — `tests/health_keepalive.rs` checks they agree.
Optionally set the GitHub repository variable `HEALTH_CHECK_URL` (Worker
origin, no trailing slash) to enable the 10-minute external monitor in
`.github/workflows/health-monitor.yml`.

---

## 2. A VM behind Caddy

The server speaks plain HTTP by design and must sit behind a TLS-terminating
reverse proxy — without one, passwords cross the network in cleartext and the
`Secure` session cookie is never sent. Bind the app to loopback
(`APP_BIND_ADDR=127.0.0.1:3000`, the default) and let Caddy own ports 80/443.

Serve the frontend and API from **one origin** so requests are same-origin,
CORS doesn't apply, and `SameSite=Lax` cookies work without opting into
cross-site sending:

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

Caddy obtains and renews the certificate itself. Run the backend under a
supervisor (systemd, or `docker run --restart unless-stopped` with the image
from `Dockerfile`) with the environment from the checklist below, and run
`sqlx migrate run` before each release that adds a migration. Keep it to
**one** process — see [the single-instance constraint](#the-single-instance-constraint).

On a VM there's no Worker cron; the process stays up on its own, so no
keepalive is needed. Point an uptime monitor at `https://<host>/api/health`.

---

## Production environment checklist

Startup fails fast with a descriptive error if a required var is missing or a
secret is shorter than 32 bytes. See `src/config.rs` and `.env.example`.

| Variable | Required | Production value / notes |
|---|---|---|
| `DATABASE_URL` | yes | Production Postgres URL. Secret. |
| `JWT_SECRET` | yes | ≥ 32 bytes, random. Rotating it signs everyone out. Secret. |
| `WEBHOOK_SECRET` | yes | ≥ 32 bytes, random. Secret. |
| `WALLET_ENCRYPTION_KEY` | yes | ≥ 32 bytes. Encrypts wallet seeds at rest — **back it up**; losing it makes every custodial wallet unrecoverable. Secret. |
| `OTP_HMAC_SECRET` | yes | ≥ 32 bytes, random, not reused elsewhere. Secret. |
| `STELLAR_SYSTEM_WALLET_ADDRESS` | yes | The system wallet's public `G...` address. |
| `PAYSTACK_SECRET_KEY` | yes | `sk_live_...` only once the business is activated for Transfers. Secret. |
| `OTP_PROVIDER` | no (`termii`) | Must be `termii` in production; `mock` logs codes instead of sending them. |
| `TERMII_API_KEY` | when `termii` | Secret. |
| `TERMII_SENDER_ID` | when `termii` | A sender ID approved in the Termii dashboard. |
| `TERMII_WEBHOOK_SECRET` | recommended | Signs `/webhooks/termii`; falls back to `TERMII_API_KEY` with a startup warning. Secret. |
| `APP_BIND_ADDR` | no (`127.0.0.1:3000`) | `0.0.0.0:3000` inside a container; loopback behind Caddy. |
| `STELLAR_HORIZON_URL` | no (testnet) | `https://horizon.stellar.org` for mainnet. |
| `STELLAR_POLL_INTERVAL_SECS` | no (`60`) | Deposit polling cadence. |
| `STELLAR_MIN_CONFIRMATIONS` | no (`1`) | Ledgers to wait before pending → available. |
| `CORS_ALLOWED_ORIGINS` | no | Exact HTTPS frontend origin(s), comma-separated. |
| `COOKIE_SECURE` | no (`true`) | Keep `true` in production. |
| `COOKIE_SAME_SITE` | no (`lax`) | `none` only for a cross-origin frontend. |
| `DB_MAX_CONNECTIONS` / `DB_MIN_CONNECTIONS` / `DB_ACQUIRE_TIMEOUT_SECS` | no | Size to your Postgres tier's connection limit. |
| `LOG_FORMAT` | no (`text`) | `json` for log aggregators. |

---

## Post-deploy verification

Replace `$HOST` with the Worker URL (or `https://pay.example.com/api` behind Caddy).

1. **Liveness** — `curl -i $HOST/health` → `200` with
   `{"status":"ok","version":"<expected version>"}`.
2. **Startup config** — `npx wrangler tail` (or your process logs) shows no
   `... is required` / `must be at least 32 bytes` errors and no
   `TERMII_WEBHOOK_SECRET` fallback warning.
3. **Database reachable** —
   `curl -i -X POST $HOST/login -H 'content-type: application/json' -d '{"email":"nobody@example.com","password":"wrong-password"}'`
   → `401 INVALID_CREDENTIALS`. A `500` means the app can't reach Postgres.
4. **CORS** — a preflight from your frontend origin succeeds:
   `curl -i -X OPTIONS $HOST/me -H "Origin: https://your-frontend.example" -H 'Access-Control-Request-Method: GET'`
   returns `access-control-allow-origin: https://your-frontend.example`.
5. **Webhook signature check** —
   `curl -i -X POST $HOST/webhooks/termii -H 'content-type: application/json' -d '{}'`
   → `403` (unsigned requests are rejected).
6. **Cron keepalive** (Cloudflare) — after ~5 minutes, Workers →
   `aframp-backend` → *Cron Events* shows successful runs.
7. **Stellar polling** — logs show the deposit worker polling every
   `STELLAR_POLL_INTERVAL_SECS` with no repeated Horizon errors.
8. **End to end** — sign up a test merchant, create a wallet, send a small
   payment to it, and confirm `GET /balance` reflects it after
   `STELLAR_MIN_CONFIRMATIONS` ledgers.
