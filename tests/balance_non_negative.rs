//! #1239 — balances::apply_delta must never drive a balance below zero.
//!
//! `apply_delta` itself has no floor check, so the guard is the DB CHECK
//! constraint proposed in `scripts/proposed_0017_balances_non_negative.sql`.
//! The constraint test is `#[ignore]`d until a maintainer applies that
//! migration; run it with `cargo test --test balance_non_negative -- --ignored`.

mod common;

use aframp::models::UpdateBalance;
use aframp::services::balances;
use uuid::Uuid;

use common::{available_balance, ensure_merchant, seed_balance, state};

#[tokio::test]
async fn positive_delta_restores_funds() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (_, merchant_id) = ensure_merchant(&app, "bal_nonneg_pos").await;
    seed_balance(&state.db, &merchant_id, "XLM", 100).await;

    balances::apply_delta(
        &state.db,
        &UpdateBalance {
            merchant_id: Uuid::parse_str(&merchant_id).unwrap(),
            asset: "XLM".into(),
            available_delta: 50,
            pending_delta: 0,
        },
    )
    .await
    .expect("positive delta must succeed");

    assert_eq!(available_balance(&state.db, &merchant_id, "XLM").await, 150);
}

#[tokio::test]
#[ignore = "requires scripts/proposed_0017_balances_non_negative.sql to be applied"]
async fn negative_delta_below_zero_is_rejected_and_balance_unchanged() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (_, merchant_id) = ensure_merchant(&app, "bal_nonneg_neg").await;
    seed_balance(&state.db, &merchant_id, "XLM", 100).await;

    let err = balances::apply_delta(
        &state.db,
        &UpdateBalance {
            merchant_id: Uuid::parse_str(&merchant_id).unwrap(),
            asset: "XLM".into(),
            available_delta: -101,
            pending_delta: 0,
        },
    )
    .await
    .expect_err("pushing available below zero must fail");

    let constraint = err
        .as_database_error()
        .and_then(|e| e.constraint())
        .map(str::to_owned);
    assert_eq!(
        constraint.as_deref(),
        Some("balances_available_non_negative"),
        "expected a descriptive CHECK violation, got {err}"
    );
    assert_eq!(available_balance(&state.db, &merchant_id, "XLM").await, 100);
}
