# Payment Requests Pagination Load Test

This document describes the load test in `tests/payment_requests_pagination_load.rs`, which verifies that cursor pagination for `GET /payment-requests` stays within a reasonable latency budget at scale.

## What the test does

- **Seeds** 50,000 payment request rows for a single merchant in a disposable test database.
- **Measures** the per-page latency of cursor pagination through `GET /payment-requests` (via `src/services/payment_requests.rs`), fetching pages until the dataset is exhausted.
- **Asserts** that each page fetch completes in **under 100ms**.

## Prerequisites

- A running PostgreSQL instance.
- A **disposable** test database — the test seeds a large volume of rows and should not be pointed at a shared or production database.
- The `TEST_DATABASE_URL` environment variable set to point at that database, e.g.:

  ```sh
  export TEST_DATABASE_URL=postgres://user:password@localhost:5432/payment_requests_load_test
  ```

## Running the test

The test is annotated with `#[ignore]`, so it does **not** run as part of the normal CI suite. Run it explicitly:

```sh
TEST_DATABASE_URL=postgres://... cargo test --test payment_requests_pagination_load -- --ignored --nocapture
```

The `--ignored` flag is required to run the `#[ignore]`-gated test, and `--nocapture` shows the per-page latency output printed during the run.

## Interpreting failures

A failure (a page exceeding the 100ms budget) typically indicates one of:

- **A missing or dropped index** on `(merchant_id, created_at, id)` — the composite index that cursor pagination relies on. Without it, each page fetch degrades to a sequential scan.
- **A query regression** in `src/services/payment_requests.rs` — for example, an added join, an unindexed filter, or a change to the cursor predicate that prevents index usage.

If the test fails, check the query plan for the pagination query (e.g. with `EXPLAIN ANALYZE`) and confirm the composite index exists before investigating the service code.

## Related documentation

See also `docs/deposit-polling-worker-load-testing.md` for the analogous load test covering the deposit polling worker.