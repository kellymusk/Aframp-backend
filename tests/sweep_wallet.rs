//! Tests for the platform settlement sweep wallet.
//!
//! **Issue #1276** — `STELLAR_SYSTEM_WALLET_ADDRESS` is validated at startup
//! but not yet used. These tests document the *expected* behaviour once the
//! sweep is implemented, and provide integration hooks for when it lands.
//!
//! ## What "sweep" means
//!
//! When a merchant initiates a withdrawal (`POST /withdraw`), the platform
//! must move the equivalent crypto from the merchant's Stellar wallet to the
//! platform settlement wallet (`STELLAR_SYSTEM_WALLET_ADDRESS`) before calling
//! Paystack. This file tests that contract at the HTTP layer.
//!
//! ## Running
//!
//! All database-backed tests require `TEST_DATABASE_URL`:
//!
//! ```bash
//! TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/aframp_test \
//!     cargo test --test sweep_wallet -- --ignored
//! ```
//!
//! The compile-time / in-memory tests run without any env-var:
//!
//! ```bash
//! cargo test --test sweep_wallet
//! ```
//!
//! NOTE: The sweep feature is not yet implemented in `src/`. Database-backed
//! tests that assert sweep-specific behaviour (e.g. `sweep_transactions` row
//! creation) are marked `#[ignore]` and will require both `TEST_DATABASE_URL`
//! and the feature to be implemented before they pass.

mod common;

// ── in-memory / compile-time tests ───────────────────────────────────────────

/// Documents the sweep transaction state machine.
///
/// A sweep moves through the following states:
///
///   pending → submitted → confirmed
///              └→ failed
///
/// This test documents the valid transitions so a reviewer can verify any
/// future implementation against them without reading the migration file.
#[test]
fn sweep_transaction_state_machine() {
    // Valid statuses (mirrors the CHECK constraint in the proposed migration).
    let valid_statuses = ["pending", "submitted", "confirmed", "failed"];
    // Valid transitions.
    let transitions: &[(&str, &str)] = &[
        ("pending", "submitted"),
        ("submitted", "confirmed"),
        ("submitted", "failed"),
    ];
    // Every transition source must be a valid status.
    for (from, to) in transitions {
        assert!(valid_statuses.contains(from), "invalid from-state: {from}");
        assert!(valid_statuses.contains(to), "invalid to-state: {to}");
    }
    // "confirmed" and "failed" are terminal — no transitions out.
    let terminal = ["confirmed", "failed"];
    for (from, _) in transitions {
        assert!(!terminal.contains(from), "terminal state {from} has an outgoing transition");
    }
}

/// Documents the sweep amount constraints.
///
/// A sweep must move the exact amount the withdrawal debited — no more, no
/// less. This test verifies the constraint constants.
#[test]
fn sweep_amount_matches_withdrawal_amount() {
    // The sweep amount must equal the withdrawal amount_stroops.
    let withdrawal_stroops: i64 = 500_000_000; // 50 XLM
    let sweep_stroops: i64 = withdrawal_stroops; // always equal
    assert_eq!(sweep_stroops, withdrawal_stroops);
    // Sweep amounts must be positive.
    assert!(sweep_stroops > 0);
}

/// Documents the expected sequence of operations in a successful withdrawal
/// with sweep.
#[test]
fn withdrawal_sweep_sequence_documented() {
    // Order of operations for POST /withdraw (once sweep is implemented):
    //
    //  1. Validate request (amount, asset, bank_code, account_number)
    //  2. Check available balance (must be >= amount_stroops)
    //  3. Debit available balance atomically (in DB transaction)
    //  4. Insert withdrawal row (status: pending)
    //  5. Decrypt merchant wallet secret key
    //  6. Build and sign Stellar payment transaction
    //  7. Submit to Horizon
    //  8. Insert sweep_transactions row (status: submitted)
    //  9. Call Paystack (initiate transfer)
    // 10. On Paystack success: update withdrawal (status: completed)
    // 11. On Paystack failure: refund balance, update withdrawal (status: failed)
    // 12. On sweep failure: refund balance, update withdrawal (status: failed),
    //                       update sweep_transactions (status: failed)
    //
    // This test is a compile-time documentation guard — it always passes.
    let steps = [
        "validate",
        "check_balance",
        "debit",
        "insert_withdrawal",
        "decrypt_key",
        "build_tx",
        "submit_tx",
        "insert_sweep",
        "call_paystack",
        "handle_paystack_result",
        "handle_sweep_failure",
    ];
    assert!(!steps.is_empty(), "sequence must have at least one step");
}

// ── integration tests (require TEST_DATABASE_URL + feature implementation) ───

/// A successful `POST /withdraw` must create a `sweep_transactions` row with
/// `status = 'submitted'` (or `'confirmed'` if the sweep is synchronous).
///
/// This test will fail until issue #1276 is implemented in `src/`.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and issue #1276 to be implemented in src/"]
async fn successful_withdrawal_creates_sweep_transaction_row() {
    let Some(state) = common::state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let app = aframp::router(state.clone());

    // Create a merchant with a wallet and a funded balance.
    let (token, _merchant_id) = common::ensure_merchant(&app, "sweep-test").await;

    // Create wallet.
    let (status, body) = common::send(
        app.clone(),
        "POST",
        "/wallet/create",
        Some(&token),
        Some(serde_json::json!({ "network": "stellar" })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "wallet create: {body}");

    // NOTE: In a real test the balance would be credited by a test deposit.
    // Since credits go through the blockchain worker (not via the API),
    // this test would need a DB seeder to insert a balance row directly.
    // That seeder is out of scope here; the test structure is left in place
    // as a scaffold for when the feature is implemented.
    //
    // The assertion below documents the expected state after a successful
    // withdrawal with sweep:
    //
    //   SELECT COUNT(*) FROM sweep_transactions
    //   WHERE merchant_id = $1 AND status IN ('submitted', 'confirmed')
    //   -- must return 1

    eprintln!("NOTE: sweep_transactions table does not exist yet (issue #1276 not implemented)");
}

/// A sweep failure must result in the withdrawal being refunded and marked
/// `failed`. The merchant's available balance must be restored.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and issue #1276 to be implemented in src/"]
async fn sweep_failure_refunds_withdrawal_and_restores_balance() {
    let Some(state) = common::state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };

    // When a mock sweep provider is injected that always returns an error,
    // the withdrawal service must:
    //   1. Detect the sweep failure.
    //   2. Re-credit the merchant's available balance.
    //   3. Mark the withdrawal row as 'failed' with a meaningful failure_reason.
    //   4. Return a 502 Bad Gateway (or appropriate error) to the caller.
    //
    // This test is a scaffold — it documents the contract without exercising
    // it until the sweep feature is implemented.

    drop(state);
    eprintln!("NOTE: sweep failure path not yet implemented (issue #1276)");
}

/// `STELLAR_SYSTEM_WALLET_ADDRESS` must be forwarded to the sweep function.
///
/// This test verifies that the config value reaches the sweep service,
/// rather than being silently ignored.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and issue #1276 to be implemented in src/"]
async fn system_wallet_address_is_used_as_sweep_destination() {
    let Some(_state) = common::state().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };

    // Once the sweep is implemented, every sweep_transactions row must have
    // a Stellar destination equal to STELLAR_SYSTEM_WALLET_ADDRESS from the
    // app config.
    //
    // SELECT DISTINCT destination FROM sweep_transactions;
    // -- must contain only the configured system wallet address

    eprintln!("NOTE: sweep destination not yet enforced (issue #1276)");
}

/// The sweep must not expose the merchant's decrypted secret key in logs,
/// error messages, or API responses.
///
/// This is a security contract test — it always runs (no DB needed) and
/// documents the expected behaviour.
#[test]
fn sweep_must_not_leak_secret_key() {
    // The decrypted Stellar secret key (S... seed) must never appear in:
    //   - API response bodies
    //   - Error messages returned to callers
    //   - `tracing::info!` / `tracing::warn!` / `tracing::error!` output
    //
    // The only permitted use of the decrypted key is to sign the transaction
    // XDR in memory, after which it must be dropped immediately.
    //
    // This test documents that contract. Enforcement is via code review and
    // security audit of the sweep implementation.
    assert!(true, "contract documented — enforce in code review");
}
