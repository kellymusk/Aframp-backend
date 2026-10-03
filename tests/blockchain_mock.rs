/// Issue #1111 — blockchain module refactor: MockBlockchainListener tests
///
/// These tests verify the expected behaviour of the `MockBlockchainListener`
/// that should live in `src/blockchain/mock.rs` after the refactor described
/// in `docs/proposals/1111-blockchain-traits-module.md`.
///
/// Because the protected `src/` has not been updated yet, this file defines
/// its own local copies of the trait and mock to:
///
/// 1. Show exactly what the `mock.rs` implementation should look like.
/// 2. Serve as living acceptance tests that a maintainer can validate after
///    the `src/` refactor is applied.
///
/// Once `src/blockchain/mock.rs` is merged, the local definitions below can
/// be replaced with:
///
/// ```rust
/// use aframp::blockchain::{BlockchainListener, DetectedDeposit};
/// use aframp::blockchain::mock::MockBlockchainListener;
/// ```

// ---------------------------------------------------------------------------
// Local mirror of the post-refactor public interface
// (remove once src/blockchain/traits.rs and src/blockchain/mock.rs exist)
// ---------------------------------------------------------------------------

mod blockchain_iface {
    use async_trait::async_trait;

    /// Mirrors `src/blockchain/traits.rs` post-refactor.
    #[derive(Debug, Clone, PartialEq)]
    pub struct DetectedDeposit {
        pub tx_hash: String,
        pub destination: String,
        pub amount_stroops: i64,
        pub asset: String,
        pub confirmations: i32,
        pub memo: Option<String>,
    }

    /// Mirrors `src/blockchain/traits.rs` post-refactor.
    #[async_trait]
    pub trait BlockchainListener: Send + Sync {
        async fn fetch_deposits(
            &self,
            addresses: &[String],
        ) -> Result<Vec<DetectedDeposit>, String>;
    }

    // -----------------------------------------------------------------------
    // MockBlockchainListener — mirrors src/blockchain/mock.rs post-refactor
    // -----------------------------------------------------------------------

    /// A no-op `BlockchainListener` for use in tests.
    ///
    /// Returns an empty deposit list by default, allowing test code to confirm
    /// that the worker loop runs without panicking when there is nothing to
    /// process.
    pub struct MockBlockchainListener;

    #[async_trait]
    impl BlockchainListener for MockBlockchainListener {
        async fn fetch_deposits(
            &self,
            _addresses: &[String],
        ) -> Result<Vec<DetectedDeposit>, String> {
            Ok(vec![])
        }
    }

    /// A configurable mock that returns a pre-set list of deposits.
    /// Useful for testing worker logic that processes incoming deposits.
    pub struct FixedDepositListener {
        pub deposits: Vec<DetectedDeposit>,
    }

    #[async_trait]
    impl BlockchainListener for FixedDepositListener {
        async fn fetch_deposits(
            &self,
            _addresses: &[String],
        ) -> Result<Vec<DetectedDeposit>, String> {
            Ok(self.deposits.clone())
        }
    }

    /// A mock that always returns an error.
    /// Useful for testing error-handling paths in the worker.
    pub struct FailingListener {
        pub error: String,
    }

    #[async_trait]
    impl BlockchainListener for FailingListener {
        async fn fetch_deposits(
            &self,
            _addresses: &[String],
        ) -> Result<Vec<DetectedDeposit>, String> {
            Err(self.error.clone())
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod mock_blockchain_listener_tests {
    use super::blockchain_iface::*;

    // -----------------------------------------------------------------------
    // MockBlockchainListener (no-op)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn mock_returns_empty_deposit_list() {
        let listener = MockBlockchainListener;
        let result = listener
            .fetch_deposits(&["GWALLET...ADDRESS".to_string()])
            .await;

        assert!(result.is_ok(), "mock listener should never error");
        assert!(
            result.unwrap().is_empty(),
            "mock listener should return no deposits by default"
        );
    }

    #[tokio::test]
    async fn mock_returns_empty_for_empty_address_slice() {
        let listener = MockBlockchainListener;
        let result = listener.fetch_deposits(&[]).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn mock_returns_empty_for_multiple_addresses() {
        let listener = MockBlockchainListener;
        let addresses = vec![
            "GWALLET1...".to_string(),
            "GWALLET2...".to_string(),
            "GWALLET3...".to_string(),
        ];
        let result = listener.fetch_deposits(&addresses).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    /// Verifies that `MockBlockchainListener` can be used as a
    /// `&dyn BlockchainListener`, the pattern used by the worker.
    #[tokio::test]
    async fn mock_usable_as_trait_object() {
        let listener: &dyn BlockchainListener = &MockBlockchainListener;
        let result = listener.fetch_deposits(&["GADDR...".to_string()]).await;
        assert!(result.is_ok());
    }

    /// Verifies that `MockBlockchainListener` can be boxed as
    /// `Box<dyn BlockchainListener>`, the heap-allocated pattern used in
    /// `AppState`.
    #[tokio::test]
    async fn mock_usable_as_boxed_trait_object() {
        let listener: Box<dyn BlockchainListener> = Box::new(MockBlockchainListener);
        let result = listener.fetch_deposits(&["GADDR...".to_string()]).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // FixedDepositListener
    // -----------------------------------------------------------------------

    fn make_deposit(tx_hash: &str, amount_stroops: i64) -> DetectedDeposit {
        DetectedDeposit {
            tx_hash: tx_hash.to_string(),
            destination: "GDEST...WALLET".to_string(),
            amount_stroops,
            asset: "XLM".to_string(),
            confirmations: 1,
            memo: None,
        }
    }

    #[tokio::test]
    async fn fixed_listener_returns_configured_deposits() {
        let deposits = vec![
            make_deposit("hash_a", 10_000_000),
            make_deposit("hash_b", 25_000_000),
        ];
        let listener = FixedDepositListener {
            deposits: deposits.clone(),
        };

        let result = listener
            .fetch_deposits(&["GDEST...WALLET".to_string()])
            .await
            .expect("fixed listener should succeed");

        assert_eq!(result.len(), 2, "should return all configured deposits");
        assert_eq!(result[0].tx_hash, "hash_a");
        assert_eq!(result[0].amount_stroops, 10_000_000);
        assert_eq!(result[1].tx_hash, "hash_b");
        assert_eq!(result[1].amount_stroops, 25_000_000);
    }

    #[tokio::test]
    async fn fixed_listener_with_no_deposits_returns_empty() {
        let listener = FixedDepositListener { deposits: vec![] };
        let result = listener
            .fetch_deposits(&["GDEST...WALLET".to_string()])
            .await
            .expect("fixed listener should succeed");
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn fixed_listener_returns_same_deposits_regardless_of_address() {
        // The mock doesn't filter by address — it's a test fixture, not real
        // Horizon logic. This matches what MockProvider does for payments.
        let deposit = make_deposit("hash_x", 50_000_000);
        let listener = FixedDepositListener {
            deposits: vec![deposit],
        };

        let result_a = listener.fetch_deposits(&["GADDR_A...".to_string()]).await;
        let result_b = listener.fetch_deposits(&["GADDR_B...".to_string()]).await;

        assert_eq!(
            result_a.unwrap().len(),
            result_b.unwrap().len(),
            "fixed listener returns the same fixtures regardless of address filter"
        );
    }

    // -----------------------------------------------------------------------
    // FailingListener
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn failing_listener_propagates_error_string() {
        let listener = FailingListener {
            error: "horizon temporarily unavailable".to_string(),
        };
        let result = listener.fetch_deposits(&["GADDR...".to_string()]).await;

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "horizon temporarily unavailable",
            "error message should be forwarded verbatim"
        );
    }

    // -----------------------------------------------------------------------
    // Trait object polymorphism
    // -----------------------------------------------------------------------

    /// Demonstrates the post-refactor usage pattern: the worker holds a
    /// `Arc<dyn BlockchainListener>` and can be given either a real
    /// `StellarListener` or a `MockBlockchainListener` without changing any
    /// worker code.
    #[tokio::test]
    async fn different_listeners_satisfy_same_trait_object() {
        let listeners: Vec<Box<dyn BlockchainListener>> = vec![
            Box::new(MockBlockchainListener),
            Box::new(FixedDepositListener { deposits: vec![] }),
            Box::new(FailingListener {
                error: "test".into(),
            }),
        ];

        // All must be callable as `&dyn BlockchainListener`
        for listener in &listeners {
            let _result = listener.fetch_deposits(&["GADDR...".to_string()]).await;
        }
    }
}
