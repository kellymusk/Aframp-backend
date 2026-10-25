//! Canary for the integration-test setup itself. Every other file in `tests/`
//! depends on `common::state()`; if that helper ever regresses back to
//! silently skipping, or `TEST_DATABASE_URL` goes missing in CI, this test is
//! the one that turns the run red instead of letting it pass on zero coverage.

mod common;

#[tokio::test]
async fn test_database_is_configured_and_migrated() {
    let state = match common::try_state().await {
        Ok(state) => state,
        Err(err) => panic!("integration tests cannot run: {err}"),
    };

    let one: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(&state.db)
        .await
        .expect("test database should answer a trivial query");
    assert_eq!(one, 1);

    // The newest migration's table must exist — proves migrations actually
    // ran against this database rather than it just being reachable.
    let otp_table: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('public.otp_challenges')::text")
            .fetch_one(&state.db)
            .await
            .expect("catalog lookup should succeed");
    assert_eq!(
        otp_table.as_deref(),
        Some("otp_challenges"),
        "migrations have not been applied to TEST_DATABASE_URL"
    );
}

#[tokio::test]
async fn missing_database_url_is_an_error_not_a_skip() {
    // Checked without touching the process environment (tests run in
    // parallel): the helper's error for a missing URL must be a real error
    // that explains how to fix it, not a `None` a caller could skip on.
    let message = common::TestDbError::MissingUrl.to_string();
    assert!(message.contains("TEST_DATABASE_URL"), "{message}");
    assert!(message.contains("CONTRIBUTING.md"), "{message}");
}
