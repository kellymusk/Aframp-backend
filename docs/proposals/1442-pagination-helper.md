# Proposal: shared cursor-parsing helper (#1442)

## Problem
Every list handler (withdrawals, payment requests, transactions, admin) repeats:

```rust
let cursor = match params.cursor.as_deref() {
    Some(raw) => Some(Cursor::decode(raw).ok_or_else(|| bad_request(...))?),
    None => None,
};
```

8+ copies mean the error code/message can drift between endpoints.

## Proposal
Add to `src/pagination.rs`:

```rust
pub fn parse_cursor(raw: Option<&str>) -> ApiResult<Option<Cursor>> {
    raw.map(|r| {
        Cursor::decode(r)
            .ok_or_else(|| bad_request(ErrorCode::InvalidCursor, "invalid cursor"))
    })
    .transpose()
}
```

Handlers become `let cursor = parse_cursor(params.cursor.as_deref())?;`.
`Page::new` construction stays as-is.

## Tests
`tests/` should cover: valid cursor → `Some`, malformed → `400`, missing → `None`.

## Rollout
Mechanical replacement per handler; no API behaviour change.
