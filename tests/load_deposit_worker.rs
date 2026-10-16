//! Load test harness for the deposit worker (issue #1088).
//!
//! Registers 100+ merchants/wallets in the test DB, points the worker at a
//! mock Horizon server that returns empty responses, and asserts a single
//! `poll_once` cycle completes within `2 * STELLAR_POLL_INTERVAL_SECS`.
//!
//! Performance regression threshold: a full poll cycle over 100 wallets must
//! finish within 2x the configured poll interval. If this test starts failing
//! it means Horizon fetching has become serialized/slow enough to overrun the
//! poll interval and should be parallelized.

use std::time::{Duration, Instant};

use httpmock::prelude::*;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Number of merchants/wallets to register for the load test.
const MERCHANT_COUNT: usize = 100;

/// Poll interval used for the assertion window (seconds).
const POLL_INTERVAL_SECS: u64 = 5;

/// Build a test DB pool. Uses `TEST_DATABASE_URL` (falls back to
/// `DATABASE_URL`) so it can run against the same test DB as the rest of the
/// suite. Returns `None` when no DB is configured so the test can skip
/// gracefully in environments without Postgres.
async fn test_pool() -> Option<PgPool> {
    let url = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .ok()?;

    PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .ok()
}

/// Register `count` merchants, each with a single wallet, and return the
/// wallet ids. Uses a unique suffix so repeated runs don't collide.
async fn seed_merchants_and_wallets(pool: &PgPool, count: usize) -> Vec<i64> {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let mut wallet_ids = Vec::with_capacity(count);

    for i in 0..count {
        let merchant_id: i64 = sqlx::query_scalar(
            "INSERT INTO merchants (name, email, created_at) \
             VALUES ($1, $2, NOW()) RETURNING id",
        )
        .bind(format!("load-merchant-{suffix}-{i}"))
        .bind(format!("load-{suffix}-{i}@example.test"))
        .fetch_one(pool)
        .await
        .expect("failed to insert merchant");

        let wallet_id: i64 = sqlx::query_scalar(
            "INSERT INTO wallets (merchant_id, stellar_account, created_at) \
             VALUES ($1, $2, NOW()) RETURNING id",
        )
        .bind(merchant_id)
        .bind(format!("GLOAD{suffix}{i:04}"))
        .fetch_one(pool)
        .await
        .expect("failed to insert wallet");

        wallet_ids.push(wallet_id);
    }

    wallet_ids
}

/// Mock Horizon server that returns empty, well-formed responses for the
/// endpoints the deposit worker polls.
fn mock_horizon() -> MockServer {
    let server = MockServer::start();

    // Empty payments page for any account.
    server.mock(|when, then| {
        when.method(GET).path_matches(Regex::new(r"^/accounts/.*/payments$").unwrap());
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"_embedded":{"records":[]},"_links":{}}"#);
    });

    // Empty account detail response.
    server.mock(|when, then| {
        when.method(GET).path_matches(Regex::new(r"^/accounts/.*$").unwrap());
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"id":"","balances":[],"_links":{}}"#);
    });

    server
}

#[tokio::test]
async fn deposit_worker_poll_once_handles_100_merchants_within_budget() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping load test: no TEST_DATABASE_URL/DATABASE_URL configured");
        return;
    };

    let wallet_ids = seed_merchants_and_wallets(&pool, MERCHANT_COUNT).await;
    assert_eq!(wallet_ids.len(), MERCHANT_COUNT);

    let horizon = mock_horizon();

    // Point the worker at the mock Horizon server for this cycle.
    std::env::set_var("STELLAR_HORIZON_URL", horizon.base_url());
    std::env::set_var("STELLAR_POLL_INTERVAL_SECS", POLL_INTERVAL_SECS.to_string());

    let budget = Duration::from_secs(2 * POLL_INTERVAL_SECS);

    let started = Instant::now();
    let result = crate::services::deposit_worker::poll_once(&pool).await;
    let elapsed = started.elapsed();

    assert!(
        result.is_ok(),
        "poll_once returned an error: {:?}",
        result.err()
    );

    eprintln!(
        "deposit worker poll_once over {} wallets took {:?} (budget {:?})",
        MERCHANT_COUNT, elapsed, budget
    );

    assert!(
        elapsed <= budget,
        "poll_once over {} wallets took {:?}, exceeding the {}s budget (2 * poll_interval_secs)",
        MERCHANT_COUNT,
        elapsed,
        budget.as_secs()
    );
}
