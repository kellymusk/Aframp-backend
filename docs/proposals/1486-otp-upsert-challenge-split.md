# Proposal: Split `otp.rs::upsert_challenge()` into named sub-functions

**Issue:** #1486
**Category:** Refactor
**Target Branch:** `dev-backend`
**File:** `src/services/otp.rs`

## Summary

`upsert_challenge()` currently handles three distinct cases in a single ~90-line function:

1. An existing challenge is still within its cooldown window → reject the request.
2. An existing challenge exists but is past its cooldown → refresh it in place.
3. No existing challenge exists → insert a new row.

Each branch has different SQL and different business logic, which makes the function hard to read and hard to test in isolation. This proposal splits the function into two named sub-functions — `refresh_existing_challenge` and `insert_new_challenge` — plus a thin dispatcher `upsert_challenge` that selects between them (or rejects).

## Proposed function signatures

```rust
/// Dispatcher: decides whether to reject, refresh, or insert.
///
/// Looks up any existing challenge for the given `(user_id, purpose)` pair.
/// - If one exists and is still within its cooldown window, returns
///   `Err(OtpError::CooldownActive { retry_after })`.
/// - If one exists and is past its cooldown, delegates to
///   [`refresh_existing_challenge`].
/// - If none exists, delegates to [`insert_new_challenge`].
pub async fn upsert_challenge(
    db: &PgPool,
    user_id: Uuid,
    purpose: OtpPurpose,
    code_hash: &str,
    expires_at: DateTime<Utc>,
    cooldown_secs: i64,
    hourly_cap: i64,
    now: DateTime<Utc>,
) -> Result<OtpChallenge, OtpError>;

/// Refresh an existing challenge row in place.
///
/// Reuses the existing row: updates `code_hash`, `expires_at`, and
/// `last_sent_at`, and increments `send_count`. Because the row already
/// exists, this path **does not** insert a new row and therefore **does
/// not** count against the hourly cap — the cap is enforced only on
/// insertion (see [`insert_new_challenge`]). The cooldown check has
/// already been performed by the caller.
async fn refresh_existing_challenge(
    tx: &mut Transaction<'_, Postgres>,
    challenge_id: Uuid,
    code_hash: &str,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<OtpChallenge, OtpError>;

/// Insert a brand-new challenge row.
///
/// Creates a new row and therefore **counts against the hourly cap**.
/// Before inserting, the number of rows created for this
/// `(user_id, purpose)` within the trailing hour is counted; if it is at
/// or above `hourly_cap`, returns `Err(OtpError::HourlyCapExceeded)`.
async fn insert_new_challenge(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    purpose: OtpPurpose,
    code_hash: &str,
    expires_at: DateTime<Utc>,
    hourly_cap: i64,
    now: DateTime<Utc>,
) -> Result<OtpChallenge, OtpError>;
```

## SQL per branch

### Dispatcher lookup (`upsert_challenge`)

```sql
SELECT id, last_sent_at
FROM otp_challenges
WHERE user_id = $1 AND purpose = $2
ORDER BY created_at DESC
LIMIT 1;
```

### Cooldown rejection (inside `upsert_challenge`)

No SQL — pure comparison of `now - last_sent_at` against `cooldown_secs`. Returns `OtpError::CooldownActive { retry_after }`.

### Refresh (`refresh_existing_challenge`)

```sql
UPDATE otp_challenges
SET code_hash    = $2,
    expires_at   = $3,
    last_sent_at = $4,
    send_count   = send_count + 1
WHERE id = $1
RETURNING *;
```

### Insert (`insert_new_challenge`)

Hourly-cap check:

```sql
SELECT COUNT(*)
FROM otp_challenges
WHERE user_id = $1
  AND purpose = $2
  AND created_at >= $3;   -- $3 = now - interval '1 hour'
```

Insert:

```sql
INSERT INTO otp_challenges
    (id, user_id, purpose, code_hash, expires_at, last_sent_at, send_count, created_at)
VALUES
    ($1, $2, $3, $4, $5, $6, 1, $7)
RETURNING *;
```

## Cooldown and hourly-cap rules

- **Cooldown** applies to *both* refresh and insert paths. It is evaluated in the dispatcher against the most recent existing row's `last_sent_at`. If `now - last_sent_at < cooldown_secs`, the request is rejected with `OtpError::CooldownActive { retry_after }` and no SQL write occurs.
- **Hourly cap** applies to the *insert* path only. It counts rows created in the trailing hour for the same `(user_id, purpose)`. If `count >= hourly_cap`, the request is rejected with `OtpError::HourlyCapExceeded` and no row is inserted.
- **Refresh does not count against the hourly cap.** Refreshing reuses the existing row and only bumps `send_count`; it never creates a new row, so it cannot push the caller over the cap. This is intentional: the cap limits *new* challenges, not resends of an already-issued challenge.

## Behaviour differences: refresh vs. insert

| Aspect | `refresh_existing_challenge` | `insert_new_challenge` |
| --- | --- | --- |
| Row lifecycle | Reuses existing row (UPDATE) | Creates new row (INSERT) |
| Counts against hourly cap | **No** | **Yes** |
| Hourly-cap check performed | No | Yes |
| `send_count` | Incremented | Initialised to `1` |
| `created_at` | Unchanged | Set to `now` |
| `last_sent_at` | Updated to `now` | Set to `now` |
| Cooldown check | Performed by dispatcher before call | Performed by dispatcher before call |

## Doc-comment wording

The doc comments on the two sub-functions must make the cap distinction explicit:

- `refresh_existing_challenge`: *"Reuses the existing row … does not count against the hourly cap — the cap is enforced only on insertion."*
- `insert_new_challenge`: *"Creates a new row and therefore counts against the hourly cap."*

These wordings are included verbatim in the signatures above.

## Testing plan

Add tests in `tests/otp_rate_limit.rs` covering each branch explicitly:

1. **Cooldown rejection** — seed a challenge with `last_sent_at = now - 10s`, call `upsert_challenge` with `cooldown_secs = 60`; assert `Err(OtpError::CooldownActive { .. })` and that the row is unchanged.
2. **Challenge refresh** — seed a challenge with `last_sent_at = now - 120s`, call with `cooldown_secs = 60`; assert `Ok`, that the same `id` is returned, `send_count` incremented, `code_hash`/`expires_at` updated, and that the hourly-cap counter is unaffected.
3. **Fresh insertion** — no existing row; call `upsert_challenge`; assert `Ok`, a new `id`, `send_count == 1`, and that the hourly-cap counter increments.
4. **Hourly cap on insert** — seed `hourly_cap` rows within the trailing hour; assert `Err(OtpError::HourlyCapExceeded)` and no new row.
5. **Refresh bypasses hourly cap** — seed `hourly_cap` rows within the trailing hour *plus* an existing challenge past cooldown; assert refresh succeeds despite the cap being reached.

## Migration / compatibility

No public API change: `upsert_challenge` keeps its signature and return type. The two sub-functions are private (`async fn`, no `pub`). All callers remain unchanged.