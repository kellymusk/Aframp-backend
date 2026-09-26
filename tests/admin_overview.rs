mod common;

use common::state;

/// Overview must not observe uncommitted writes from another transaction —
/// the read batch is framed by BEGIN/COMMIT so counts stay consistent.
#[tokio::test]
async fn admin_overview_ignores_uncommitted_writes() {
    let Some(state) = state().await else {
        return;
    };

    let before = aframp::services::admin::overview_in_transaction(&state.db)
        .await
        .expect("overview before");

    let mut tx = state.db.begin().await.expect("begin writer tx");
    sqlx::query(
        "INSERT INTO users (email, password_hash, name, phone_number, phone_verified)
         VALUES ($1, 'x', 'Half Written', $2, true)",
    )
    .bind(format!("half-written-{}@example.com", uuid::Uuid::new_v4().simple()))
    .bind(format!("+2347{:010}", (uuid::Uuid::new_v4().as_u128() as u64) % 10_000_000_000))
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
