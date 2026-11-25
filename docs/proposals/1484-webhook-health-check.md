# Proposal: Per-Merchant Webhook Delivery Failure Health Check (#1484)

**Issue:** #1484 — Enhancement: add per-merchant webhook delivery failure rate metric and admin alert
**Category:** Observability
**Target Branch:** `dev-backend`
**Status:** Proposed

## Problem

Merchant webhooks currently fail silently. Rows in `webhook_deliveries` with `status = 'failed'` accumulate whenever a merchant's endpoint is unreachable, but there is no metric, endpoint, or alert that surfaces this to the operator. A merchant whose webhook URL goes down misses all `payment.confirmed` notifications with no indication that anything is wrong.

## Goal

Provide an admin-visible health check that lists merchants whose webhook endpoint has been failing for a run of consecutive delivery attempts, so the operator can proactively reach out or investigate.

## Scope

- New admin endpoint `GET /admin/webhooks/health`.
- Query logic over `webhook_deliveries` (in `src/services/merchant_webhooks.rs`).
- Route wiring and authorization in `src/api/admin.rs`.
- Documentation updates in `API.md` and `openapi.yaml`.
- Integration test seeding consecutive failures.

Out of scope: automatic disabling of webhooks, retry backoff changes, or outbound alerting (email/Slack). Those can build on this endpoint later.

## Consecutive-Failure Threshold (X)

**Threshold X = 5.**

Rationale:

- A single failure is common and usually transient (timeout, brief 5xx). Alerting on it would be noisy.
- The existing retry policy already retries a delivery several times; by the time a merchant has 5 *consecutive failed deliveries* (across distinct delivery attempts/events), the endpoint has been unreachable long enough to be actionable.
- 5 is small enough to catch a down endpoint quickly and large enough to avoid false positives from transient blips.

The threshold is defined as a constant `WEBHOOK_HEALTH_FAILURE_THRESHOLD: i64 = 5` in `src/services/merchant_webhooks.rs` so it can be tuned in one place. The endpoint reports merchants with `consecutive_failures >= X`.

## Definition of "Consecutive Failures"

For each merchant, order that merchant's `webhook_deliveries` rows by `created_at DESC` (tie-break on `id DESC`) and count the leading run of rows with `status = 'failed'` until the first non-failed row (e.g. `'delivered'` / `'success'` / `'pending'`). That count is the merchant's current consecutive-failure streak. A merchant is "unhealthy" when the streak is `>= X`.

Only the *trailing* (most recent) run counts — an old outage that has since recovered must not keep a merchant listed.

## SQL

The query groups by merchant and computes the trailing consecutive-failure streak. Using a window function to find the most recent non-failed delivery per merchant, then counting failures after it:

```sql
WITH last_success AS (
    SELECT
        merchant_id,
        MAX(created_at) AS last_ok_at
    FROM webhook_deliveries
    WHERE status <> 'failed'
    GROUP BY merchant_id
),
trailing_failures AS (
    SELECT
        d.merchant_id,
        COUNT(*)          AS consecutive_failures,
        MAX(d.created_at) AS last_failure_at
    FROM webhook_deliveries d
    LEFT JOIN last_success s
        ON s.merchant_id = d.merchant_id
    WHERE d.status = 'failed'
      AND (s.last_ok_at IS NULL OR d.created_at > s.last_ok_at)
    GROUP BY d.merchant_id
)
SELECT
    t.merchant_id,
    m.webhook_url       AS url,
    t.consecutive_failures,
    t.last_failure_at
FROM trailing_failures t
JOIN merchants m ON m.id = t.merchant_id
WHERE t.consecutive_failures >= $1   -- WEBHOOK_HEALTH_FAILURE_THRESHOLD
ORDER BY t.consecutive_failures DESC, t.last_failure_at ASC;
```

Notes:

- `last_success` finds the timestamp of the most recent non-failed delivery per merchant. If a merchant has never had a non-failed delivery, `last_ok_at` is `NULL` and *all* its failed rows count toward the streak.
- `trailing_failures` counts only failed rows newer than that last non-failed delivery, giving the trailing streak.
- The join to `merchants` supplies the `url` for the response. If `webhook_url` is stored elsewhere, adjust the join accordingly.
- The threshold is passed as a bound parameter (`$1`) rather than interpolated.
- An index on `webhook_deliveries (merchant_id, status, created_at)` is recommended to keep this query cheap as the table grows.

## Response Shape — `GET /admin/webhooks/health`

**Auth:** Admin role required (same guard as the other `/admin/*` routes in `src/api/admin.rs`). Non-admin / unauthenticated requests receive `401`/`403` per existing middleware.

**Method / Path:** `GET /admin/webhooks/health`

**Success:** `200 OK`

```json
{
  "threshold": 5,
  "merchants": [
    {
      "merchant_id": "mrc_123",
      "url": "https://merchant.example.com/webhooks/payments",
      "consecutive_failures": 7,
      "last_failure_at": "2025-01-15T09:42:11Z"
    }
  ]
}
```

Field definitions:

| Field | Type | Description |
|-------|------|-------------|
| `threshold` | integer | The configured consecutive-failure threshold `X` (currently `5`). |
| `merchants` | array | Merchants whose trailing consecutive-failure streak is `>= threshold`. Empty array when all merchants are healthy. |
| `merchants[].merchant_id` | string | Merchant identifier. |
| `merchants[].url` | string | The merchant's configured webhook URL. |
| `merchants[].consecutive_failures` | integer | Length of the trailing run of failed deliveries. |
| `merchants[].last_failure_at` | string (RFC 3339) | Timestamp of the most recent failed delivery. |

Ordering: `consecutive_failures` descending, then `last_failure_at` ascending (longest-broken first).

**Errors:** `401` unauthenticated, `403` non-admin, `500` on query failure.

## Implementation Outline

1. `src/services/merchant_webhooks.rs`
   - Add `pub const WEBHOOK_HEALTH_FAILURE_THRESHOLD: i64 = 5;`
   - Add `pub async fn list_unhealthy_merchants(pool, threshold) -> Result<Vec<WebhookHealthEntry>, _>` running the SQL above.
   - Define `WebhookHealthEntry { merchant_id, url, consecutive_failures, last_failure_at }`.
2. `src/api/admin.rs`
   - Add handler `get_webhook_health` that calls the service with the threshold constant and returns the response shape above.
   - Register route `GET /admin/webhooks/health` behind the existing admin auth guard.
3. Docs: update `API.md` and `openapi.yaml` with the endpoint, auth, and schema.

## Test

**Location:** integration tests for the admin API (e.g. `tests/admin_webhooks_health.rs`).

**Scenario:** seed a merchant with 5 consecutive failed deliveries → `GET /admin/webhooks/health` lists the merchant.

Steps:

1. Seed a merchant with a known `webhook_url`.
2. Insert 5 `webhook_deliveries` rows for that merchant, all with `status = 'failed'`, with increasing `created_at` timestamps.
3. Call `GET /admin/webhooks/health` as an admin.
4. Assert `200 OK`.
5. Assert the response `merchants` array contains exactly one entry for the seeded merchant, with:
   - `merchant_id` equal to the seeded id,
   - `url` equal to the seeded URL,
   - `consecutive_failures == 5`,
   - `last_failure_at` equal to the most recent failed delivery's timestamp.
6. Assert `threshold == 5`.

**Additional cases (recommended):**

- **Below threshold:** a merchant with 4 consecutive failures is *not* listed.
- **Recovery resets streak:** a merchant with 5 failures followed by a `delivered` row is *not* listed (trailing streak is 0).
- **Non-admin:** a non-admin caller receives `403`.

## Rollout / Risk

- Read-only endpoint; no schema migration required (index recommended but optional).
- No change to existing delivery/retry behavior.
- Threshold is a single constant, easy to tune or later make configurable.
- Future work: emit a metric/gauge from the same query and wire an operator alert on non-empty results.