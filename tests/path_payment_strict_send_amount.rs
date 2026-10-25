/// Issue #1110 — path_payment_strict_send: use `amount_received` not `amount`
///
/// ## Background
///
/// Horizon's payments feed exposes three amount-related fields for
/// `path_payment_strict_send` operations:
///
/// | Field            | Meaning                                        |
/// |------------------|------------------------------------------------|
/// | `amount`         | Amount **sent** (input side, what the sender pays) |
/// | `amount_received`| Amount **received** (destination, what arrived) |
/// | `destination_min`| Minimum amount the sender was willing to deliver |
///
/// The current deposit worker (`src/blockchain/stellar.rs`) reads `amount` for
/// all operation types.  For `path_payment_strict_receive` this is correct —
/// `amount` is the received amount.  For `path_payment_strict_send`, however,
/// `amount` is the **sent** (input) amount, which can differ significantly from
/// what the destination actually received after the swap.
///
/// ## Example
///
/// Suppose a sender swaps 10 XLM → cNGN via a DEX path:
///
/// ```text
/// amount          = "10.0000000"   // 10 XLM sent
/// amount_received = "14500.0000000" // 14,500 cNGN received (correct credit)
/// ```
///
/// Reading `amount` credits the merchant 10 stroops-equivalent of the wrong
/// asset; reading `amount_received` credits 14,500 — the actual deposit.
///
/// ## Required fix (in src/blockchain/stellar.rs)
///
/// 1. Add `amount_received` to `OperationRecord`:
///    ```rust
///    #[serde(default)]
///    amount_received: Option<String>,
///    ```
///
/// 2. In `record_to_deposit`, for `path_payment_strict_send`, use
///    `amount_received` with a fallback to `amount`:
///    ```rust
///    "path_payment_strict_send"
///        if record.destination.as_deref() == Some(address) =>
///    {
///        record.amount_received.as_deref().or(record.amount.as_deref())
///    }
///    ```
///
/// The tests below encode the expected behaviour after that fix.
/// They do **not** compile against the production source (which lives in the
/// protected `src/` tree); they are specification tests that document the
/// correct contract and serve as acceptance criteria for the implementer.

// ---------------------------------------------------------------------------
// Specification / contract tests for #1110
//
// These tests describe the expected behaviour of the deposit-detection logic
// once the fix is applied.  They are written as plain unit tests so they can
// be reviewed without a running database.
// ---------------------------------------------------------------------------

/// The contract for `path_payment_strict_send` amount field selection.
///
/// When Horizon returns both `amount` (sent) and `amount_received` (arrived),
/// the deposit worker must credit `amount_received` — not `amount`.
#[cfg(test)]
mod path_payment_strict_send_amount_selection {
    /// Represents the relevant slice of a Horizon `OperationRecord` as it
    /// arrives from the JSON response.  This mirrors the fields that
    /// `src/blockchain/stellar.rs` should deserialize.
    #[derive(Debug, Clone)]
    struct OperationRecord {
        op_type: &'static str,
        transaction_successful: bool,
        /// `to` — used by `payment` and `path_payment_strict_receive`.
        to: Option<&'static str>,
        /// `destination` — used by `path_payment_strict_send`.
        destination: Option<&'static str>,
        /// `amount` — sent amount for strict_send, received amount for all others.
        amount: Option<&'static str>,
        /// `amount_received` — the actual received amount for strict_send.
        amount_received: Option<&'static str>,
    }

    /// Minimal deposit record produced by the worker.
    #[derive(Debug, PartialEq)]
    struct Deposit {
        amount_stroops: i64,
    }

    /// The fixed `record_to_deposit` logic (post-patch).
    fn record_to_deposit(rec: &OperationRecord, address: &str) -> Option<Deposit> {
        if !rec.transaction_successful {
            return None;
        }

        let raw_amount: Option<&str> = match rec.op_type {
            "payment" | "path_payment_strict_receive" if rec.to == Some(address) => rec.amount,
            "path_payment_strict_send" if rec.destination == Some(address) => {
                // FIX: prefer amount_received over amount for strict_send.
                rec.amount_received.or(rec.amount)
            }
            _ => None,
        };

        raw_amount.map(|s| Deposit {
            amount_stroops: parse_to_stroops(s),
        })
    }

    fn parse_to_stroops(s: &str) -> i64 {
        let mut parts = s.splitn(2, '.');
        let whole: i64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        let frac_str = parts.next().unwrap_or("");
        let frac_padded = format!("{frac_str:0<7}");
        let frac: i64 = frac_padded[..7].parse().unwrap_or(0);
        whole * 10_000_000 + frac
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// For `path_payment_strict_send`, when `amount_received` is present it
    /// MUST be used — not `amount` (which is the sent/input side of the swap).
    #[test]
    fn strict_send_uses_amount_received_not_amount() {
        let addr = "GDEST...WALLET";
        let rec = OperationRecord {
            op_type: "path_payment_strict_send",
            transaction_successful: true,
            to: None,
            destination: Some(addr),
            amount: Some("10.0000000"),         // sent (XLM)
            amount_received: Some("14500.0000000"), // received (cNGN)
        };

        let deposit = record_to_deposit(&rec, addr)
            .expect("strict_send to the wallet must produce a deposit");

        // Must credit 14 500 cNGN worth of stroops, NOT 10 XLM worth.
        assert_eq!(
            deposit.amount_stroops,
            145_000_000_000,
            "strict_send should credit amount_received (14500 → 145_000_000_000 stroops), \
             not amount (10 → 100_000_000)"
        );
    }

    /// When `amount_received` is absent (older Horizon response or operation
    /// variant that omits it), fall back to `amount` rather than discarding the
    /// deposit entirely.
    #[test]
    fn strict_send_falls_back_to_amount_when_amount_received_missing() {
        let addr = "GDEST...WALLET";
        let rec = OperationRecord {
            op_type: "path_payment_strict_send",
            transaction_successful: true,
            to: None,
            destination: Some(addr),
            amount: Some("10.0000000"),
            amount_received: None, // older response — field absent
        };

        let deposit = record_to_deposit(&rec, addr)
            .expect("should fall back to amount when amount_received is absent");

        assert_eq!(
            deposit.amount_stroops, 100_000_000,
            "fallback to amount when amount_received is missing"
        );
    }

    /// `payment` operations must NOT be affected — they still use `amount`
    /// (which for a regular payment IS the received amount).
    #[test]
    fn regular_payment_still_uses_amount_field() {
        let addr = "GDEST...WALLET";
        let rec = OperationRecord {
            op_type: "payment",
            transaction_successful: true,
            to: Some(addr),
            destination: None,
            amount: Some("5.0000000"),
            amount_received: None,
        };

        let deposit = record_to_deposit(&rec, addr)
            .expect("regular payment must produce a deposit");

        assert_eq!(deposit.amount_stroops, 50_000_000);
    }

    /// `path_payment_strict_receive` already receives the correct amount in
    /// `amount` — it must not be changed by this fix.
    #[test]
    fn strict_receive_uses_amount_field_unchanged() {
        let addr = "GDEST...WALLET";
        let rec = OperationRecord {
            op_type: "path_payment_strict_receive",
            transaction_successful: true,
            to: Some(addr),
            destination: None,
            amount: Some("25.0000000"),
            amount_received: None,
        };

        let deposit = record_to_deposit(&rec, addr)
            .expect("path_payment_strict_receive must produce a deposit");

        assert_eq!(deposit.amount_stroops, 250_000_000);
    }

    /// A strict_send op where `destination` does NOT match the wallet must
    /// produce no deposit — even if both `amount` and `amount_received` are set.
    #[test]
    fn strict_send_ignored_for_different_destination() {
        let addr = "GDEST...WALLET";
        let rec = OperationRecord {
            op_type: "path_payment_strict_send",
            transaction_successful: true,
            to: None,
            destination: Some("GOTHER...ACCOUNT"),
            amount: Some("10.0000000"),
            amount_received: Some("14500.0000000"),
        };

        assert!(
            record_to_deposit(&rec, addr).is_none(),
            "strict_send to a different destination must not produce a deposit"
        );
    }

    /// A failed transaction must never produce a deposit, regardless of operation
    /// type or amount fields present.
    #[test]
    fn failed_transaction_produces_no_deposit() {
        let addr = "GDEST...WALLET";
        let rec = OperationRecord {
            op_type: "path_payment_strict_send",
            transaction_successful: false,
            to: None,
            destination: Some(addr),
            amount: Some("10.0000000"),
            amount_received: Some("14500.0000000"),
        };

        assert!(
            record_to_deposit(&rec, addr).is_none(),
            "failed transactions must never produce a deposit"
        );
    }

    /// Verify the stroop parser handles the boundary of a 7-decimal amount.
    #[test]
    fn parse_to_stroops_handles_various_formats() {
        assert_eq!(parse_to_stroops("1.0000000"), 10_000_000);
        assert_eq!(parse_to_stroops("0.0000001"), 1);
        assert_eq!(parse_to_stroops("14500.0000000"), 145_000_000_000);
        assert_eq!(parse_to_stroops("0.5"), 5_000_000);
    }
}
