use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;

/// Maximum backoff for unfunded wallets (10 minutes).  Each consecutive 404
/// doubles the backoff up to this ceiling.
const MAX_BACKOFF: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct DetectedDeposit {
    pub tx_hash: String,
    pub destination: String,
    pub amount_stroops: i64,
    pub asset: String,
    pub confirmations: i32,
    /// The parent transaction's memo, if any — used to correlate a deposit to
    /// a specific payment request rather than just "something arrived."
    pub memo: Option<String>,
}

#[async_trait]
pub trait BlockchainListener: Send + Sync {
    async fn fetch_deposits(&self, addresses: &[String]) -> Result<Vec<DetectedDeposit>, String>;
}

pub struct StellarListener {
    pub horizon_url: String,
    /// Tracks the last time each unfunded wallet was polled so the worker can
    /// apply exponential backoff instead of hammering Horizon every cycle.
    unfunded_backoff: Mutex<HashMap<String, UnfundedState>>,
}

#[derive(Debug, Clone)]
struct UnfundedState {
    /// When the next poll for this address should be attempted.
    next_poll_at: Instant,
    /// Consecutive 404 count — controls the backoff doubling.
    consecutive_404s: u32,
}

impl StellarListener {
    pub fn new(horizon_url: String) -> Self {
        Self {
            horizon_url,
            unfunded_backoff: Mutex::new(HashMap::new()),
        }
    }

    /// Returns `true` if the address is still in its backoff window and should
    /// be skipped for this poll cycle.
    fn should_skip(&self, address: &str) -> bool {
        let map = self.unfunded_backoff.lock().unwrap();
        map.get(address)
            .is_some_and(|s| s.next_poll_at > Instant::now())
    }

    /// Record a consecutive 404 for the address and compute the next allowed
    /// poll time using exponential backoff:  base * 2^n  capped at MAX_BACKOFF.
    fn record_unfunded(&self, address: &str) {
        let mut map = self.unfunded_backoff.lock().unwrap();
        let state = map.entry(address.to_string()).or_insert(UnfundedState {
            next_poll_at: Instant::now(),
            consecutive_404s: 0,
        });
        state.consecutive_404s = state.consecutive_404s.saturating_add(1);
        // base interval = 30s, doubled each consecutive 404, capped at MAX_BACKOFF.
        let backoff_secs = 30u64
            .saturating_mul(2u64.saturating_pow(state.consecutive_404s.saturating_sub(1).min(10)));
        let backoff = Duration::from_secs(backoff_secs).min(MAX_BACKOFF);
        state.next_poll_at = Instant::now() + backoff;
        tracing::debug!(
            %address,
            consecutive_404s = state.consecutive_404s,
            backoff_secs = backoff.as_secs(),
            "wallet unfunded — backing off"
        );
    }

    /// Clear backoff state when the wallet becomes funded (receives a 200).
    fn clear_backoff(&self, address: &str) {
        let mut map = self.unfunded_backoff.lock().unwrap();
        if map.remove(address).is_some() {
            tracing::debug!(%address, "wallet now funded — clearing backoff");
        }
    }
}

#[async_trait]
impl BlockchainListener for StellarListener {
    async fn fetch_deposits(&self, addresses: &[String]) -> Result<Vec<DetectedDeposit>, String> {
        let mut deposits = Vec::new();
        for address in addresses {
            // Skip wallets that are in their unfunded backoff window.
            if self.should_skip(address) {
                continue;
            }
            match fetch_for_address(&self.horizon_url, address).await {
                Ok(FetchResult::Funded(found)) => {
                    self.clear_backoff(address);
                    deposits.extend(found);
                }
                Ok(FetchResult::Unfunded) => {
                    self.record_unfunded(address);
                }
                Err(err) => {
                    // One bad/unreachable address must not block deposit detection
                    // for every other wallet in this poll cycle.
                    tracing::warn!(error = %err, %address, "failed to fetch deposits for address");
                }
            }
        }
        Ok(deposits)
    }
}

#[derive(Debug, Deserialize)]
struct PaymentsPage {
    #[serde(rename = "_embedded")]
    embedded: Embedded,
}

#[derive(Debug, Deserialize)]
struct Embedded {
    records: Vec<OperationRecord>,
}

#[derive(Debug, Deserialize)]
struct OperationRecord {
    #[serde(rename = "type")]
    op_type: String,
    transaction_successful: bool,
    transaction_hash: String,
    #[serde(default)]
    to: Option<String>,
    /// `path_payment_strict_send` uses `destination` instead of `to`.
    #[serde(default)]
    destination: Option<String>,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    amount_sent: Option<String>,
    #[serde(default)]
    starting_balance: Option<String>,
    #[serde(default)]
    asset_type: Option<String>,
    #[serde(default)]
    asset_code: Option<String>,
    /// `claimable_balance_created` uses `asset` (the full asset string) rather
    /// than `asset_type` / `asset_code`.
    #[serde(default)]
    asset: Option<String>,
    /// `claimable_balance_created` uses `amount` for the total amount.
    #[serde(default)]
    claimant: Option<ClaimantInfo>,
    #[serde(default)]
    transaction: Option<EmbeddedTransaction>,
}

/// Horizon wraps each claimant in the `claimants` array.  For the
/// `claimable_balance_created` operation the per-claimant `destination`
/// is the wallet the balance is claimable by.
#[derive(Debug, Deserialize)]
struct ClaimantInfo {
    destination: String,
}

#[derive(Debug, Deserialize)]
struct EmbeddedTransaction {
    #[serde(default)]
    memo: Option<String>,
}

/// Distinguishes a funded account (200 OK) from an unfunded one (404) so the
/// caller can apply backoff for the latter.
enum FetchResult {
    Funded(Vec<DetectedDeposit>),
    Unfunded,
}

/// Polls Horizon's per-account payments feed for one wallet address and maps
/// successful incoming operations to deposits. A brand-new wallet's very
/// first funding always arrives as a `create_account` operation (Stellar
/// rejects `payment` ops to accounts that don't exist on-ledger yet), so both
/// operation types are handled here, not just `payment`.
///
/// `claimable_balance_created` and `path_payment_strict_send` are also handled —
/// the former delivers funds via claimable balances (not payment ops), and the
/// latter uses a `destination` field instead of `to`.
async fn fetch_for_address(horizon_url: &str, address: &str) -> Result<FetchResult, String> {
    let url = format!(
        "{}/accounts/{address}/payments?order=desc&limit=20&include_failed=false&join=transactions",
        horizon_url.trim_end_matches('/')
    );

    let response = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // Account has no ledger history yet (never funded) — nothing to detect.
        return Ok(FetchResult::Unfunded);
    }

    let page: PaymentsPage = response
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let deposits = page
        .embedded
        .records
        .into_iter()
        .filter_map(|r| record_to_deposit(r, address))
        .collect();
    Ok(FetchResult::Funded(deposits))
}

/// Maps a single Horizon operation record to a `DetectedDeposit` if it
/// represents an incoming payment to `address`.  Returns `None` for failed
/// transactions, unrelated operations, or unparseable amounts.
///
/// Extracted as a free function so unit tests can exercise each operation type
/// without spinning up an HTTP mock.
fn record_to_deposit(record: OperationRecord, address: &str) -> Option<DetectedDeposit> {
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

    Some(DetectedDeposit {
        tx_hash: record.transaction_hash,
        destination: address.to_string(),
        amount_stroops,
        asset,
        confirmations: 1,
        memo,
    })
}

/// Converts a Stellar decimal amount string (up to 7 fractional digits) to stroops.
fn parse_amount_to_stroops(amount: &str) -> Result<i64, String> {
    let mut parts = amount.splitn(2, '.');
    let whole = parts.next().unwrap_or("0");
    let frac = parts.next().unwrap_or("");
    if frac.len() > 7 {
        return Err(format!("unexpected precision in amount: {amount}"));
    }
    let frac_padded = format!("{frac:0<7}");
    let whole_stroops: i64 = whole.parse().map_err(|_| format!("invalid amount: {amount}"))?;
    let frac_stroops: i64 = frac_padded
        .parse()
        .map_err(|_| format!("invalid amount: {amount}"))?;
    Ok(whole_stroops * 10_000_000 + frac_stroops)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            to: None,
            destination: None,
            account: None,
            amount: None,
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
    // #921 — unfunded wallet exponential backoff
    // --------------------------------------------------------------------------

    #[test]
    fn backoff_skips_address_after_first_404() {
        let listener = StellarListener::new("https://horizon-testnet.stellar.org".into());
        let addr = "GUNFUNDED...ADDRESS";

        // Before any 404, address is not skipped.
        assert!(!listener.should_skip(addr));

        listener.record_unfunded(addr);

        // After one 404, the address should be in backoff (next_poll_at is in
        // the future).
        assert!(listener.should_skip(addr));
    }

    #[test]
    fn backoff_cleared_when_wallet_becomes_funded() {
        let listener = StellarListener::new("https://horizon-testnet.stellar.org".into());
        let addr = "GUNFUNDED...ADDRESS";

        listener.record_unfunded(addr);
        assert!(listener.should_skip(addr));

        listener.clear_backoff(addr);
        assert!(!listener.should_skip(addr));
    }

    #[test]
    fn backoff_increases_with_consecutive_404s() {
        let listener = StellarListener::new("https://horizon-testnet.stellar.org".into());
        let addr = "GUNFUNDED...ADDRESS";

        // First 404 → backoff = 30s
        listener.record_unfunded(addr);
        {
            let map = listener.unfunded_backoff.lock().unwrap();
            let state = map.get(addr).unwrap();
            assert_eq!(state.consecutive_404s, 1);
        }

        // Second 404 → consecutive count increases
        listener.record_unfunded(addr);
        {
            let map = listener.unfunded_backoff.lock().unwrap();
            let state = map.get(addr).unwrap();
            assert_eq!(state.consecutive_404s, 2);
        }

        // Still in backoff
        assert!(listener.should_skip(addr));
    }

    #[test]
    fn backoff_independent_per_address() {
        let listener = StellarListener::new("https://horizon-testnet.stellar.org".into());
        let addr_a = "GADDR...A";
        let addr_b = "GADDR...B";

        listener.record_unfunded(addr_a);
        assert!(listener.should_skip(addr_a));
        assert!(!listener.should_skip(addr_b));

        listener.clear_backoff(addr_a);
        assert!(!listener.should_skip(addr_a));
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
}
