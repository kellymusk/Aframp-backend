# Proposal: #1126 — map Query rejection for `limit=abc` onto API error shape

> Follow-up for maintainers. Contributor tests in `tests/limit_params.rs`
> already assert `400` (never `500`) for `?limit=abc` on `/transactions`,
> `/withdrawals`, and `/payment-requests`. When the body is JSON they also
> require `{ "error", "code": "INVALID_PARAMETERS" }` per API.md.

## Problem

`ListParams { limit: Option<i64> }` is extracted with axum's `Query`. A value
like `abc` fails serde deserialization and axum answers with a **plain-text**
`400` body — bypassing the documented `{ "error", "code" }` contract the same
way a mislabeled JSON body used to (see `middleware::require_json_content_type`).

## Proposed fix (protected path — maintainer apply)

Parse `limit` as `Option<String>` (or a custom newtype) and map parse failures
through `bad_request_field`:

```rust
fn parse_limit(raw: Option<&str>, default: i64) -> Result<i64, (StatusCode, Json<ApiError>)> {
    match raw {
        None => Ok(default),
        Some(s) => {
            let n: i64 = s.parse().map_err(|_| {
                bad_request_field("limit", "limit must be an integer")
            })?;
            Ok(n.clamp(1, 200))
        }
    }
}
```

Apply the same helper in `transactions::list`, `withdrawals::list`, and
`payment_requests::list`. After that lands, the JSON branch of
`assert_bad_limit` in `tests/limit_params.rs` becomes the only path.
