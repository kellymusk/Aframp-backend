# Proposal: cheap ETag for `GET /payment-requests` (#1446)

## Problem
`list()` fetches the page and serialises it before `etag::conditional_json` hashes the body. A matching `If-None-Match` saves only response bytes, not the DB query or serialisation.

## Proposal
Derive the ETag from a cheap aggregate before loading rows:
```sql
SELECT MAX(updated_at), COUNT(*) FROM payment_requests WHERE merchant_id = $1;
```
ETag = `W/"<merchant_id>-<max_updated_at_epoch_ms>-<count>-<query params hash>"`. If it matches `If-None-Match`, return `304` immediately; otherwise run the existing page query.

## Notes
- Requires every mutation to bump `updated_at` (and deletions are caught by `count`).
- An index on `(merchant_id, updated_at)` keeps the aggregate an index-only scan.
- Weak ETag, since the body is not hashed.
