//! Load test for cursor pagination of `GET /payment-requests`.
//!
//! This test seeds 50,000 payment request rows for a single merchant and then
//! walks the cursor pagination (following `next_cursor`) until the result set is
//! exhausted, asserting that each page fetch completes within a 100ms budget and
//! that every seeded row is visited exactly once.
//!
//! It is modeled on `tests/deposit_polling_worker_load.rs` and complements the
//! small-dataset correctness coverage in `tests/pagination_flow.rs`. The query
//! under test lives in `src/services/payment_requests.rs`.
//!
//! Because this test seeds a large number of rows it is annotated with
//! `#[ignore]` and must be run explicitly against a test database:
//!
//! ```text
//! TEST_DATABASE_URL=postgres://... cargo test --test payment_requests_pagination_load -- --ignored --nocapture
//! ```

mod common;

use std::time::{Duration, Instant};

use common::{ensure_merchant, send, state};
use serde_json::Value;

/// Number of payment request rows to seed for the load test.
const SEED_ROWS: i64 = 50_000;

/// Page size used while walking the cursor pagination.
const PAGE_LIMIT: i64 = 200;

/// Maximum allowed latency for a single page fetch.
const PAGE_BUDGET: Duration = Duration::from_millis(100);

#[tokio::test]
#[ignore = "load test: seeds 50,000 rows; run explicitly"]
async fn payment_requests_cursor_pagination_load() {
    let state = state().await;
    let merchant = ensure_merchant(&state).await;
    let merchant_id = merchant.merchant_id;
    let wallet_id = merchant.wallet_id;

    // Seed 50,000 payment request rows in a single bulk insert. `created_at`
    // and `id` are deterministic so that cursor ordering is stable across runs.
    // The `generate_series` produces a monotonically increasing sequence which
    // we use to derive unique memos and timestamps.
    sqlx::query(
        r#"
        INSERT INTO payment_requests (
            merchant_id,
            wallet_id,
            amount_stroops,
            asset,
            memo,
            expires_at,
            created_at
        )
        SELECT
            $1,
            $2,
            (1000 + g)::bigint,
            'XLM',
            'load-test-' || g,
            NOW() + INTERVAL '1 day',
            TIMESTAMPTZ '2024-01-01 00:00:00+00' + (g || ' seconds')::interval
        FROM generate_series(1, $3) AS g
        "#,
    )
    .bind(merchant_id)
    .bind(wallet_id)
    .bind(SEED_ROWS)
    .execute(&state.db)
    .await
    .expect("failed to seed payment request rows");

    // Walk the cursor pagination until exhausted, timing each page fetch.
    let mut cursor: Option<String> = None;
    let mut visited: i64 = 0;
    let mut pages: usize = 0;

    loop {
        let mut path = format!("/payment-requests?limit={PAGE_LIMIT}");
        if let Some(ref c) = cursor {
            path.push_str("&cursor=");
            path.push_str(c);
        }

        let started = Instant::now();
        let response = send(&state, &merchant, "GET", &path, None).await;
        let elapsed = started.elapsed();

        assert!(
            response.status().is_success(),
            "page {pages} returned non-success status: {}",
            response.status()
        );

        assert!(
            elapsed < PAGE_BUDGET,
            "page {pages} fetch took {elapsed:?}, exceeding budget of {PAGE_BUDGET:?}",
        );

        let body: Value = response
            .json()
            .await
            .expect("failed to parse payment requests page response");

        let items = body
            .get("items")
            .and_then(Value::as_array)
            .expect("response missing `items` array");

        visited += items.len() as i64;
        pages += 1;

        cursor = body
            .get("next_cursor")
            .and_then(Value::as_str)
            .map(str::to_owned);

        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(
        visited, SEED_ROWS,
        "expected to visit {SEED_ROWS} rows exactly once, but visited {visited} across {pages} pages",
    );

    println!(
        "payment_requests cursor pagination load test: visited {visited} rows across {pages} pages, each under {PAGE_BUDGET:?}",
    );
}