# Retry-After Header on 429 Responses — Design Note

**Issue:** [#1273](https://github.com/kellymusk/Aframp-backend/issues/1273)  
**Status:** Pending implementation in `src/`

---

## Background

The OTP service returns `OtpError::RateLimited` when a client exceeds either
the per-phone resend cooldown (`RESEND_COOLDOWN_SECS = 60`) or the hourly send
cap (`MAX_SENDS_PER_HOUR = 5`). This error maps to a `429 Too Many Requests`
response via the `too_many_requests` helper in `src/error.rs`.

Currently the `429` body is:

```json
{ "error": "too many requests, please try again shortly", "code": "TOO_MANY_REQUESTS" }
```

No `Retry-After` header is included.

---

## Why This Matters

[RFC 6585 §4](https://datatracker.ietf.org/doc/html/rfc6585#section-4) recommends
including a `Retry-After` header with `429` responses so clients know exactly
how long to wait before retrying. Without it:

- Mobile apps and retry libraries apply arbitrary (often aggressive) back-off.
- Users are presented with vague "try again shortly" messaging instead of a
  countdown they can act on.
- Frontend polling loops may retry too early, wasting OTP sends and locking
  accounts faster.

The API contract in `API.md` documents that the 60-second cooldown exists, but
clients cannot programmatically determine how many seconds remain.

---

## Proposed Change (requires `src/` write access)

The change is confined to two files:

### 1. `src/services/otp.rs`

`OtpError::RateLimited` should carry the number of seconds the client must
wait:

```rust
#[derive(Debug, thiserror::Error)]
pub enum OtpError {
    /// Fired when the client hits the resend cooldown or hourly cap.
    /// `retry_after_secs` is how long the caller should wait before retrying.
    #[error("too many requests, please try again shortly")]
    RateLimited { retry_after_secs: u64 },
    // … other variants unchanged …
}
```

In `upsert_challenge`, the cooldown branch calculates:

```rust
let elapsed = (Utc::now() - last_sent_at).num_seconds().max(0) as u64;
let retry_after_secs = RESEND_COOLDOWN_SECS as u64 - elapsed.min(RESEND_COOLDOWN_SECS as u64);
return Err(OtpError::RateLimited { retry_after_secs });
```

When the hourly cap is hit, the remaining window is not readily available from
a single `COUNT` query. A safe approximation is to return the full cooldown:

```rust
return Err(OtpError::RateLimited { retry_after_secs: RESEND_COOLDOWN_SECS as u64 });
```

A more precise value can be derived by fetching the oldest send in the window:

```sql
SELECT EXTRACT(EPOCH FROM (created_at + interval '1 hour' - now()))
FROM otp_challenges
WHERE phone_number = $1 AND purpose = $2 AND created_at > now() - interval '1 hour'
ORDER BY created_at ASC LIMIT 1
```

### 2. `src/error.rs`

The `too_many_requests` helper should accept an optional `retry_after_secs`:

```rust
pub fn too_many_requests(
    code: ErrorCode,
    message: &str,
    retry_after_secs: Option<u64>,
) -> impl IntoResponse {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(ApiError { error: message.into(), code, field: None }),
    ).into_response();

    if let Some(secs) = retry_after_secs {
        response.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            axum::http::HeaderValue::from_str(&secs.to_string())
                .expect("u64 is always a valid header value"),
        );
    }
    response
}
```

All existing callers of `too_many_requests` pass `None` for `retry_after_secs`
and are unaffected.

The `OtpError → ApiError` mapping in the auth handler thread unwraps
`RateLimited { retry_after_secs }` and forwards the value:

```rust
OtpError::RateLimited { retry_after_secs } => {
    too_many_requests(ErrorCode::TooManyRequests, "too many requests, please try again shortly", Some(retry_after_secs))
}
```

---

## API Contract (already documented in API.md)

Once implemented, a rate-limited OTP response looks like:

```
HTTP/1.1 429 Too Many Requests
Retry-After: 47
Content-Type: application/json

{ "error": "too many requests, please try again shortly", "code": "TOO_MANY_REQUESTS" }
```

The value of `Retry-After` is the number of **whole seconds** the client should
wait before re-POSTing `/signup` or `/login`. It is always a non-negative
integer (never negative; `0` means "retry immediately").

Client guidance:

```js
if (res.status === 429) {
    const retryAfter = parseInt(res.headers.get('Retry-After') ?? '60', 10);
    scheduleRetry(retryAfter * 1000); // back off for exactly this long
}
```

---

## Test Coverage

A companion test file `tests/retry_after_otp.rs` (added alongside this doc)
documents the expected observable behaviour from outside `src/`:

- A `429` response to `/signup` or `/login` within the cooldown window **MUST**
  include a `Retry-After` header.
- The header value **MUST** be a non-negative integer string.
- The header value **MUST** be `≤ RESEND_COOLDOWN_SECS` (60).
- Responses for other error codes (`400`, `401`, `404`) **MUST NOT** include
  `Retry-After`.

See `tests/retry_after_otp.rs` for the test definitions.

---

## References

- [RFC 6585 §4 — 429 Too Many Requests](https://datatracker.ietf.org/doc/html/rfc6585#section-4)
- [`src/services/otp.rs`](../src/services/otp.rs) — `OtpError::RateLimited`, `upsert_challenge`
- [`src/error.rs`](../src/error.rs) — `too_many_requests` helper
- [`API.md`](../API.md) — `TOO_MANY_REQUESTS` error code documentation
