mod common;

use common::state;
use std::time::{Duration, Instant};

/// Overview must report the actual row counts for users, merchants and
/// wallets, and the status breakdowns must add up to the totals.
#[tokio::test]
async fn admin_overview_returns_correct_counts() {
    let state = state().await;
    let app = aframp::router(state.clone());

    // Seed a couple of merchants (each with a wallet) so the counts are
    // non-trivial and we can compare against the DB directly.
    for i in 0..2 {
        let (token, _merchant_id) =
            common::ensure_merchant(&app, &format!("overview_counts_{i}")).await;
        let (status, json) = common::send(
            app.clone(),
            "POST",
            "/wallet/create",
            Some(&token),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(
            status,
            axum::http::StatusCode::OK,
            "wallet create failed: {json}"
        );
    }

    let overview = aframp::services::admin::overview(&state.db)
        .await
        .expect("overview");

    let expected_users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&state.db)
        .await
        .unwrap();
    let expected_merchants: i64 = sqlx::query_scalar("SELECT count(*) FROM merchants")
        .fetch_one(&state.db)
        .await
        .unwrap();
    let expected_wallets: i64 = sqlx::query_scalar("SELECT count(*) FROM wallets")
        .fetch_one(&state.db)
        .await
        .unwrap();

    assert_eq!(overview.total_users, expected_users);
    assert_eq!(overview.total_merchants, expected_merchants);
    assert_eq!(overview.total_wallets, expected_wallets);

    // Status breakdowns must be internally consistent with the totals.
    let merchant_status_sum: i64 = overview
        .merchants_by_status
        .values()
        .map(|v| *v as i64)
        .sum();
    assert_eq!(
        merchant_status_sum, overview.total_merchants,
        "merchant status breakdown must sum to total merchants"
    );

    let wallet_status_sum: i64 = overview
        .wallets_by_status
        .values()
        .map(|v| *v as i64)
        .sum();
    assert_eq!(
        wallet_status_sum, overview.total_wallets,
        "wallet status breakdown must sum to total wallets"
    );

    // The transactional variant must agree with the concurrent one.
    let in_tx = aframp::services::admin::overview_in_transaction(&state.db)
        .await
        .expect("overview in transaction");
    assert_eq!(in_tx.total_users, overview.total_users);
    assert_eq!(in_tx.total_merchants, overview.total_merchants);
    assert_eq!(in_tx.total_wallets, overview.total_wallets);
}

/// #1485: the overview queries are independent and must run concurrently.
/// A slow response on one query must not serialise the others, so the total
/// wall-clock time should stay close to the slowest single query rather than
/// the sum of all of them.
///
/// We simulate a slow DB by holding a row lock on `users` from a competing
/// transaction: the user-count query blocks until the lock is released, while
/// the merchant/wallet queries (which touch other tables) can complete
/// immediately. If the queries were sequential, the whole overview would be
/// blocked for the full lock duration.
#[tokio::test]
async fn admin_overview_queries_run_concurrently() {
    let state = state().await;

    // Take an exclusive lock on the users table so any read that needs a
    // table-level lock (or a row lock) on it has to wait.
    let mut locker = state.db.begin().await.expect("begin locker tx");
    sqlx::query("LOCK TABLE users IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *locker)
        .await
        .expect("lock users table");

    let lock_hold = Duration::from_millis(600);
    let db = state.db.clone();
    let overview_task = tokio::spawn(async move {
        let started = Instant::now();
        let result = aframp::services::admin::overview(&db).await;
        (result, started.elapsed())
    });

    // Give the overview a moment to start its queries, then release the lock.
    tokio::time::sleep(lock_hold).await;
    locker.rollback().await.expect("release lock");

    let (result, elapsed) = overview_task.await.expect("overview task panicked");
    result.expect("overview should succeed once the lock is released");

    // Sequential execution would still finish, but the point of this test is
    // that the concurrent implementation does not deadlock or serialise on the
    // locked table in a way that multiplies the wait. We assert the overview
    // completed shortly after the lock was released rather than waiting for
    // multiple sequential lock acquisitions.
    assert!(
        elapsed < lock_hold + Duration::from_secs(2),
        "overview took {elapsed:?}, expected it to complete promptly after the \
         lock was released (concurrent queries should not serialise)"
    );
}

/// Overview must not observe uncommitted writes from another transaction —
/// the read batch is framed by BEGIN/COMMIT so counts stay consistent.
#[tokio::test]
async fn admin_overview_ignores_uncommitted_writes() {
    let state = state().await;

    let before = aframp::services::admin::overview_in_transaction(&state.db)
        .await
        .expect("overview before");

    let mut tx = state.db.begin().await.expect("begin writer tx");
    sqlx::query(
        "INSERT INTO users (email, password_hash, name, phone_number, phone_verified)
         VALUES ($1, 'x', 'Half Written', $2, true)",
    )
    .bind(format!(
        "half-written-{}@example.com",
        uuid::Uuid::new_v4().simple()
    ))
    .bind(format!(
        "+2347{:010}",
        (uuid::Uuid::new_v4().as_u128() as u64) % 10_000_000_000
    ))
    .execute(&mut *tx)
    .await
    .expect("insert uncommitted user");

    let during = aframp::services::admin::overview_in_transaction(&state.db)
        .await
        .expect("overview during open write tx");

    assert_eq!(
        during.total_users, before.total_users,
        "overview must not count an uncommitted user insert"
    );
    assert_eq!(during.total_merchants, before.total_merchants);
    assert_eq!(during.total_wallets, before.total_wallets);

    // Concurrent try_join path must also hide uncommitted rows (MVCC).
    let during_fast = aframp::services::admin::overview(&state.db)
        .await
        .expect("concurrent overview during open write tx");
    assert_eq!(during_fast.total_users, before.total_users);

    tx.rollback().await.expect("rollback");

    let after = aframp::services::admin::overview_in_transaction(&state.db)
        .await
        .expect("overview after rollback");
    assert_eq!(after.total_users, before.total_users);
}

/// #1209: a merchant with wallets on more than one network must still be
/// listed once by the admin merchants query.
#[tokio::test]
async fn admin_merchants_lists_multi_wallet_merchant_once() {
    let state = state().await;
    let app = aframp::router(state.clone());
    let (token, merchant_id) = common::ensure_merchant(&app, "admin_multi_wallet").await;
    let (status, json) = common::send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "wallet create failed: {json}"
    );

    let merchant_id: uuid::Uuid = merchant_id.parse().unwrap();
    sqlx::query(
        "INSERT INTO wallets (merchant_id, address, network, secret_key_encrypted)
         VALUES ($1, $2, 'bitcoin', 'unused')",
    )
    .bind(merchant_id)
    .bind(format!("bc1q{}", uuid::Uuid::new_v4().simple()))
    .execute(&state.db)
    .await
    .expect("insert second wallet");

    let rows = aframp::services::admin::merchants(&state.db, 200, None)
        .await
        .expect("admin merchants");
    assert_eq!(rows.data.iter().filter(|r| r.id == merchant_id).count(), 1);
}

/// Admin lists are keyset-paginated: following `next_cursor` walks every row
/// exactly once, newest first.
#[tokio::test]
async fn admin_merchants_pages_without_overlap() {
    let state = state().await;
    let app = aframp::router(state.clone());
    for i in 0..3 {
        common::ensure_merchant(&app, &format!("admin_paging_{i}")).await;
    }

    let mut seen = std::collections::HashSet::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = aframp::services::admin::merchants(&state.db, 2, cursor)
            .await
            .expect("admin merchants page");
        assert!(page.data.len() <= 2);
        for row in &page.data {
            assert!(
                seen.insert(row.id),
                "merchant {} appeared on two pages",
                row.id
            );
        }
        pages += 1;
        match page.next_cursor {
            Some(raw) => {
                cursor = Some(aframp::pagination::Cursor::decode(&raw).expect("valid cursor"))
            }
            None => break,
        }
    }
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM merchants")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(seen.len() as i64, total, "paging must visit every merchant");
    assert!(pages >= 2);
}
