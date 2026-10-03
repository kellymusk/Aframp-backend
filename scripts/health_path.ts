/**
 * Canonical container keep-alive / liveness path (#1130).
 *
 * Shared contract between:
 *   - Rust server route in `src/lib.rs` (`GET` this path → `204`)
 *   - Cloudflare Worker cron in `cloudflare/worker.ts` (scheduled fetch)
 *   - `wrangler.jsonc` cron schedule (`*/5 * * * *` — every five minutes)
 *
 * If this path or the `/health` response contract changes, update all three
 * call sites and the contract test in `tests/health_keepalive.rs`.
 */
export const HEALTH_CHECK_PATH = "/health" as const;

/** How often the Worker cron must ping the path (matches wrangler triggers). */
export const HEALTH_KEEPALIVE_CRON = "*/5 * * * *" as const;

/** Alert if /health has not succeeded within this window (#1130 monitoring). */
export const HEALTH_ALERT_WINDOW_MINUTES = 10 as const;
