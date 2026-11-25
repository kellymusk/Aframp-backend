# Proposal: Add `created_by_ip` to API Key Records for Audit Trail

**Issue:** #1499
**Category:** Security – Feature
**Priority:** Medium
**Target Branch:** `dev-backend`

## Summary

Record the IP address of the merchant who created each API key by adding a nullable `created_by_ip` column to the `api_keys` table and threading the existing client-IP extractor through the `POST /api-keys` handler into the key-creation service.

## Motivation

`src/services/api_keys.rs` currently creates API key records without capturing any information about *where* the key was created. If a key is later compromised and used to initiate fraudulent withdrawals, investigators have no record of the originating IP address for the creation event. This gap makes it difficult to:

- Correlate key creation with known-bad IPs or geographies.
- Distinguish a legitimate merchant-created key from one minted by an attacker who gained dashboard access.
- Reconstruct a timeline during incident response.

The `client_ip` extraction pattern already exists in `src/client_ip.rs` and is used by the OTP challenge path, so the infrastructure to capture the requesting IP is already available and battle-tested. Reusing it here is low-risk and consistent with existing conventions.

## Design

### Schema change

Add a nullable column to the `api_keys` table:

```sql
ALTER TABLE api_keys ADD COLUMN created_by_ip inet;
```

**Type choice — `inet` vs `text`:**

- `inet` is preferred: it validates the value at the database layer, supports CIDR-aware queries (e.g. `WHERE created_by_ip << '10.0.0.0/8'`), and normalizes storage. It also rejects malformed input, which is a useful safety net.
- `text` would be simpler to write but loses validation and query ergonomics.

We recommend `inet`. If the deployment target cannot guarantee `inet` support (e.g. an unusual driver or ORM mapping), fall back to `text` — the application-level behavior is identical.

**Nullability:** The column is `NULL`-able. Pre-existing rows have no recorded IP and must not be backfilled with a fabricated value. New rows created after the migration will populate it whenever the extractor yields an IP. Consumers must treat `NULL` as "unknown / pre-migration".

**Migration:** This migration must be applied by a maintainer (per the issue note). It is additive and non-blocking on PostgreSQL (adding a nullable column without a default does not rewrite the table). No backfill is required.

### Source changes

1. **`src/models/api_key.rs`** — add an optional `created_by_ip` field to the API key model (and any relevant row/DTO structs). Map it to the `inet` column; represent it in Rust as `Option<String>` (or `Option<IpAddr>` if the existing DB layer supports it cleanly).

2. **`src/client_ip.rs`** — no changes required. The existing `client_ip` extractor is reused as-is.

3. **`src/api/api_keys.rs`** — the `POST /api-keys` handler gains the `client_ip` extractor as a handler argument (mirroring the OTP challenge path) and passes the resolved IP into the service call:

   ```rust
   async fn create_api_key(
       State(state): State<AppState>,
       client_ip: ClientIp,          // existing extractor from crate::client_ip
       Json(payload): Json<CreateApiKeyRequest>,
   ) -> Result<Json<ApiKeyResponse>, ApiError> {
       let key = api_keys::create(&state, &payload, client_ip.0).await?;
       Ok(Json(key.into()))
   }
   ```

   The exact extractor signature should match how the OTP challenge path consumes it, to keep the two call sites consistent.

4. **`src/services/api_keys.rs`** — `api_keys::create` accepts the optional IP and persists it in the `INSERT`:

   ```rust
   pub async fn create(
       state: &AppState,
       payload: &CreateApiKeyRequest,
       created_by_ip: Option<IpAddr>,
   ) -> Result<ApiKey, ApiKeyError> {
       // ... existing logic ...
       sqlx::query!(
           r#"
           INSERT INTO api_keys (/* ...existing columns... */, created_by_ip)
           VALUES (/* ... */, $N)
           RETURNING *
           "#,
           /* ... */,
           created_by_ip,
       )
       // ...
   }
   ```

   If the extractor cannot determine an IP (e.g. behind an unconfigured proxy), pass `None` rather than a placeholder.

### Response exposure

**Recommendation: admin-only.**

`created_by_ip` is audit metadata, not something the merchant needs to see on every key listing. Exposing it in the standard key response:

- Leaks internal network topology / proxy IPs to any party holding a key.
- Adds noise to a response consumed by SDKs and dashboards.
- Provides little value to the merchant, who already knows where they created the key.

Therefore:

- The public `ApiKeyResponse` returned by `POST /api-keys` and `GET /api-keys` **omits** `created_by_ip`.
- The field is surfaced only through an admin-scoped endpoint (or an admin-only field gated on an admin principal), documented as such.

If a future requirement calls for merchant-visible creation IPs, it can be added behind an explicit opt-in without a schema change.

### Testing

Add a test asserting that `POST /api-keys` records the requesting IP in the created key row:

- Issue a request with a known `X-Forwarded-For` / client IP (per how `src/client_ip.rs` resolves the IP in the test harness).
- Assert the persisted `api_keys.created_by_ip` equals the expected IP.
- Add a companion test asserting that when no IP can be resolved, the column is `NULL` and the request still succeeds.

## Documentation

- **`API.md`** — document that `created_by_ip` is captured for audit purposes and is **admin-only** (not present in the standard key response). If an admin endpoint exposes it, document the field there.
- **`openapi.yaml`** — either omit the field from the public `ApiKey` schema, or add it only to the admin schema with a description noting it is admin-only and may be `null` for keys created before the migration.

## Rollout / Backward Compatibility

- Additive, nullable column — no breaking change to existing rows or clients.
- Public API responses are unchanged.
- The migration is applied by a maintainer; application code that references `created_by_ip` must ship after the migration is in place (or be written to tolerate the column's absence during a brief window, if the deploy order requires it).

## References

- `src/services/api_keys.rs` — key creation service; gains the `created_by_ip` parameter and `INSERT` column.
- `src/api/api_keys.rs` — `POST /api-keys` handler; gains the `client_ip` extractor.
- `src/models/api_key.rs` — API key model; gains the optional `created_by_ip` field.
- `src/client_ip.rs` — existing client-IP extractor, reused unchanged (also used by the OTP challenge path).