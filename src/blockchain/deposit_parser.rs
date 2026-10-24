//! Pure deposit parsing: maps a Horizon operation record to a
//! [`DetectedDeposit`] and converts Stellar decimal amounts to stroops. No
//! I/O, so every operation type can be unit-tested without an HTTP mock.

use super::horizon::OperationRecord;
use super::stellar::DetectedDeposit;

/// Maps a single Horizon operation record to a `DetectedDeposit` if it
/// represents an incoming payment to `address`.  Returns `None` for failed
/// transactions, unrelated operations, or unparseable amounts.
///
/// Extracted as a free function so unit tests can exercise each operation type
/// without spinning up an HTTP mock.
pub(crate) fn record_to_deposit(record: OperationRecord, address: &str) -> Option<DetectedDeposit> {
    if !record.transaction_successful {
        return None;
    }

    let amount_str: Option<&str> = match record.op_type.as_str() {
        "create_account" if record.account.as_deref() == Some(address) => {
            record.starting_balance.as_deref()
        }
        // `payment` and `path_payment_strict_receive` populate `to`.
        "payment" | "path_payment_strict_receive"
            if record.to.as_deref() == Some(address) =>
        {
            record.amount.as_deref()
        }
        // `path_payment_strict_send` uses `destination` instead of `to`.
        "path_payment_strict_send"
            if record.destination.as_deref() == Some(address) =>
        {
            record.amount.as_deref()
        }
        // `claimable_balance_created`: funds delivered when claimant
        // destination matches the wallet address.
        "claimable_balance_created"
            if record.claimant.as_ref().is_some_and(|c| c.destination == address) =>
        {
            record.amount.as_deref()
        }
        _ => None,
    };
    let amount_str = amount_str?;

    let amount_stroops = match parse_amount_to_stroops(amount_str) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(
                error = %err,
                tx_hash = %record.transaction_hash,
                "skipping deposit with unparseable amount"
            );
            return None;
        }
    };

    let asset = if record.op_type == "claimable_balance_created" {
        // claimable_balance_created uses a flat `asset` string (e.g. "native" or
        // "USDC:GA...") instead of the split `asset_type` / `asset_code` fields.
        match record.asset.as_deref() {
            Some("native") | None => "XLM".to_string(),
            Some(s) => s.split(':').next().unwrap_or("unknown").to_string(),
        }
    } else {
        match record.asset_type.as_deref() {
            Some("native") | None => "XLM".to_string(),
            _ => record.asset_code.clone().unwrap_or_else(|| "unknown".into()),
        }
    };

    let memo = record.transaction.as_ref().and_then(|t| t.memo.clone());
    // Prefer the ledger on the operation record; fall back to the joined
    // transaction.  Default to 0 when Horizon omits it entirely so the worker
    // can still track the deposit (it will simply wait for the threshold).
    let ledger = record
        .ledger
        .or_else(|| record.transaction.as_ref().and_then(|t| t.ledger))
        .unwrap_or(0);

    Some(DetectedDeposit {
        tx_hash: record.transaction_hash,
        destination: address.to_string(),
        amount_stroops,
        asset,
        confirmations: 1,
        ledger,
        memo,
    })
}

/// Converts a Stellar decimal amount string (up to 7 fractional digits) to stroops.
///
/// Returns `Err` for:
/// - Empty or whitespace-only strings
/// - Negative amounts (e.g. `"-1.0"`) — a negative stroop count would corrupt
///   the balance ledger if passed through to `balances::apply_delta`.
/// - Values whose whole part overflows `i64` when multiplied by 10_000_000
///   (the stroops conversion factor).
/// - More than 7 fractional digits (Stellar only supports 7 decimal places).
pub(crate) fn parse_amount_to_stroops(amount: &str) -> Result<i64, String> {
    let trimmed = amount.trim();
    if trimmed.is_empty() {
        return Err(format!("invalid amount: {amount}"));
    }

    let mut parts = trimmed.splitn(2, '.');
    let whole = parts.next().unwrap_or("0");
    let frac = parts.next().unwrap_or("");
    if frac.len() > 7 {
        return Err(format!("unexpected precision in amount: {amount}"));
    }
    let frac_padded = format!("{frac:0<7}");

    // Parse the whole part as an unsigned 64-bit integer first so that a
    // leading minus sign is rejected before the overflow-checked multiply.
    let whole_val: u64 = whole
        .parse()
        .map_err(|_| format!("invalid amount: {amount}"))?;
    let frac_stroops: i64 = frac_padded
        .parse()
        .map_err(|_| format!("invalid amount: {amount}"))?;

    // Checked multiply: whole_val * 10_000_000 must fit in i64.
    let whole_stroops: i64 = (whole_val as i64)
        .checked_mul(10_000_000)
        .ok_or_else(|| format!("amount overflows i64: {amount}"))?;

    whole_stroops
        .checked_add(frac_stroops)
        .ok_or_else(|| format!("amount overflows i64: {amount}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockchain::horizon::{ClaimantInfo, EmbeddedTransaction};

    #[test]
    fn parses_whole_and_fractional_amounts() {
        assert_eq!(parse_amount_to_stroops("10000.0000000").unwrap(), 100_000_000_000);
        assert_eq!(parse_amount_to_stroops("0.5").unwrap(), 5_000_000);
        assert_eq!(parse_amount_to_stroops("1").unwrap(), 10_000_000);
        assert_eq!(parse_amount_to_stroops("123.4567890").unwrap(), 1_234_567_890);
    }

    #[test]
    fn rejects_excess_precision() {
        assert!(parse_amount_to_stroops("1.12345678").is_err());
    }

    // --------------------------------------------------------------------------
    // #911 — path_payment_strict_send uses `destination`, not `to`
    // --------------------------------------------------------------------------

    fn make_record(op_type: &str) -> OperationRecord {
        OperationRecord {
            op_type: op_type.to_string(),
            transaction_successful: true,
            transaction_hash: "abc123".to_string(),
            ledger: Some(1234),
            to: None,
            destination: None,
            account: None,
            amount: None,
            amount_sent: None,
            starting_balance: None,
            asset_type: Some("native".to_string()),
            asset_code: None,
            asset: None,
            claimant: None,
            transaction: None,
        }
    }

    #[test]
    fn path_payment_strict_send_matched_via_destination_field() {
        let addr = "GDESK...WALLET";
        let mut rec = make_record("path_payment_strict_send");
        rec.destination = Some(addr.to_string());
        rec.amount = Some("5.0000000".to_string());

        let deposit = record_to_deposit(rec, addr).expect("strict_send should be detected");
        assert_eq!(deposit.amount_stroops, 50_000_000);
        assert_eq!(deposit.destination, addr);
    }

    #[test]
    fn path_payment_strict_send_ignored_when_to_field_used() {
        // A strict_send op with only `to` set (not `destination`) must NOT match —
        // the old code incorrectly checked `to` for this op type.
        let addr = "GDESK...WALLET";
        let mut rec = make_record("path_payment_strict_send");
        rec.to = Some(addr.to_string());
        rec.amount = Some("5.0000000".to_string());

        assert!(record_to_deposit(rec, addr).is_none());
    }

    #[test]
    fn path_payment_strict_send_ignored_for_wrong_destination() {
        let addr = "GDESK...WALLET";
        let mut rec = make_record("path_payment_strict_send");
        rec.destination = Some("GOTHER...ACCOUNT".to_string());
        rec.amount = Some("5.0000000".to_string());

        assert!(record_to_deposit(rec, addr).is_none());
    }

    #[test]
    fn regular_payment_still_uses_to_field() {
        let addr = "GDESK...WALLET";
        let mut rec = make_record("payment");
        rec.to = Some(addr.to_string());
        rec.amount = Some("10.0000000".to_string());

        let deposit = record_to_deposit(rec, addr).expect("payment should be detected");
        assert_eq!(deposit.amount_stroops, 100_000_000);
    }

    // --------------------------------------------------------------------------
    // #916 — claimable_balance_created deposit detection
    // --------------------------------------------------------------------------

    #[test]
    fn claimable_balance_created_detected_when_claimant_matches() {
        let addr = "GDESK...WALLET";
        let mut rec = make_record("claimable_balance_created");
        rec.claimant = Some(ClaimantInfo {
            destination: addr.to_string(),
        });
        rec.amount = Some("3.5000000".to_string());

        let deposit =
            record_to_deposit(rec, addr).expect("claimable_balance_created should be detected");
        assert_eq!(deposit.amount_stroops, 35_000_000);
    }

    #[test]
    fn claimable_balance_created_ignored_when_claimant_is_other() {
        let addr = "GDESK...WALLET";
        let mut rec = make_record("claimable_balance_created");
        rec.claimant = Some(ClaimantInfo {
            destination: "GOTHER...ACCOUNT".to_string(),
        });
        rec.amount = Some("3.5000000".to_string());

        assert!(record_to_deposit(rec, addr).is_none());
    }

    #[test]
    fn claimable_balance_created_ignored_without_claimant() {
        let addr = "GDESK...WALLET";
        let mut rec = make_record("claimable_balance_created");
        rec.amount = Some("3.5000000".to_string());

        assert!(record_to_deposit(rec, addr).is_none());
    }

    // --------------------------------------------------------------------------
    // #1197 — ledger sequence carried on detected deposits
    // --------------------------------------------------------------------------

    #[test]
    fn deposit_carries_ledger_from_operation_record() {
        let addr = "GDESK...WALLET";
        let mut rec = make_record("payment");
        rec.to = Some(addr.to_string());
        rec.amount = Some("10.0000000".to_string());
        rec.ledger = Some(4242);

        let deposit = record_to_deposit(rec, addr).expect("payment should be detected");
        assert_eq!(deposit.ledger, 4242);
    }

    #[test]
    fn deposit_falls_back_to_transaction_ledger() {
        let addr = "GDESK...WALLET";
        let mut rec = make_record("payment");
        rec.to = Some(addr.to_string());
        rec.amount = Some("10.0000000".to_string());
        rec.ledger = None;
        rec.transaction = Some(EmbeddedTransaction {
            memo: None,
            ledger: Some(777),
        });

        let deposit = record_to_deposit(rec, addr).expect("payment should be detected");
        assert_eq!(deposit.ledger, 777);
    }

    #[test]
    fn deposit_defaults_ledger_to_zero_when_absent() {
        let addr = "GDESK...WALLET";
        let mut rec = make_record("payment");
        rec.to = Some(addr.to_string());
        rec.amount = Some("10.0000000".to_string());
        rec.ledger = None;

        let deposit = record_to_deposit(rec, addr).expect("payment should be detected");
        assert_eq!(deposit.ledger, 0);
    }

    #[test]
    fn failed_transaction_produces_no_deposit() {
        let addr = "GDESK...WALLET";
        let mut rec = make_record("payment");
        rec.transaction_successful = false;
        rec.to = Some(addr.to_string());
        rec.amount = Some("10.0000000".to_string());

        assert!(record_to_deposit(rec, addr).is_none());
    }

    #[test]
    fn unrelated_operation_produces_no_deposit() {
        let addr = "GDESK...WALLET";
        let rec = make_record("account_merge");

        assert!(record_to_deposit(rec, addr).is_none());
    }

    // --------------------------------------------------------------------------
    // #1041 — parse_amount_to_stroops edge cases: overflow, negative, empty,
    //          whitespace
    // --------------------------------------------------------------------------

    /// A whole-part value large enough to overflow i64 when multiplied by
    /// 10_000_000 (the stroops factor).  i64::MAX / 10_000_000 ≈ 922_337_203,
    /// so "922337204.0" is the smallest whole XLM value that overflows.
    #[test]
    fn parse_amount_overflow_returns_err() {
        // 922_337_204 * 10_000_000 > i64::MAX — must not silently truncate.
        assert!(
            parse_amount_to_stroops("922337204.0").is_err(),
            "overflow should return Err"
        );

        // Even larger values are also rejected.
        assert!(
            parse_amount_to_stroops("999999999999999999.0").is_err(),
            "very large value should return Err"
        );
    }

    /// Negative amounts (e.g. from a malformed Horizon response) must be
    /// rejected — a negative stroop count would silently corrupt the balance
    /// ledger if it slipped through.
    #[test]
    fn parse_amount_negative_returns_err() {
        assert!(
            parse_amount_to_stroops("-1.0").is_err(),
            "negative amount should return Err"
        );
        assert!(
            parse_amount_to_stroops("-0.0000001").is_err(),
            "negative fractional amount should return Err"
        );
    }

    /// An empty string is not a valid decimal and must be rejected rather than
    /// interpreted as zero or panicking.
    #[test]
    fn parse_amount_empty_string_returns_err() {
        assert!(
            parse_amount_to_stroops("").is_err(),
            "empty string should return Err"
        );
    }

    /// Whitespace-only input must be rejected.  A real Horizon amount field
    /// will never be blank, but a test or mocked response could be.
    #[test]
    fn parse_amount_whitespace_only_returns_err() {
        assert!(
            parse_amount_to_stroops("   ").is_err(),
            "whitespace-only string should return Err"
        );
        assert!(
            parse_amount_to_stroops("\t").is_err(),
            "tab-only string should return Err"
        );
        assert!(
            parse_amount_to_stroops("\n").is_err(),
            "newline-only string should return Err"
        );
    }
}
