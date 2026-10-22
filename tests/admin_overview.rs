mod common;

use common::state;

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
