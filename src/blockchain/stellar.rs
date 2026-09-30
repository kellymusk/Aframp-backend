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

/// Per-request timeout for Horizon calls, so one slow node can't stall the
/// whole poll cycle.
const HORIZON_TIMEOUT: Duration = Duration::from_secs(15);

pub struct StellarListener {
    pub horizon_url: String,
    /// Shared client: pools connections across polls and applies the timeout.
    http: reqwest::Client,
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
        Self::with_timeout(horizon_url, HORIZON_TIMEOUT)
    }

    pub fn with_timeout(horizon_url: String, timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("failed to build Horizon HTTP client");
        Self {
            horizon_url,
            http,
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
            match fetch_for_address(&self.http, &self.horizon_url, address).await {
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
async fn fetch_for_address(
    http: &reqwest::Client,
    horizon_url: &str,
    address: &str,
) -> Result<FetchResult, String> {
    let url = format!(
        "{}/accounts/{address}/payments?order=desc&limit=20&include_failed=false&join=transactions",
        horizon_url.trim_end_matches('/')
    );

    let response = http.get(&url).send().await.map_err(|e| e.to_string())?;
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
///
/// Returns `Err` for:
/// - Empty or whitespace-only strings
/// - Negative amounts (e.g. `"-1.0"`) — a negative stroop count would corrupt
///   the balance ledger if passed through to `balances::apply_delta`.
/// - Values whose whole part overflows `i64` when multiplied by 10_000_000
///   (the stroops conversion factor).
/// - More than 7 fractional digits (Stellar only supports 7 decimal places).
fn parse_amount_to_stroops(amount: &str) -> Result<i64, String> {
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
    #[tokio::test]
    async fn a_hanging_horizon_request_times_out_and_the_poll_continues() {
        const SLOW: &str = "GSLOW";
        const FAST: &str = "GFAST";
        let app = axum::Router::new().route(
            "/accounts/{address}/payments",
            axum::routing::get(|axum::extract::Path(address): axum::extract::Path<String>| async move {
                if address == SLOW {
                    // Longer than the listener's timeout: simulates a stuck node.
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                axum::Json(serde_json::json!({
                    "_embedded": { "records": [{
                        "type": "payment",
                        "transaction_successful": true,
                        "transaction_hash": "fast-tx",
                        "to": FAST,
                        "amount": "10.0000000",
                        "asset_type": "native"
                    }] }
                }))
            }),
        );
        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", server.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(server, app).await.unwrap() });

        let listener = StellarListener::with_timeout(url, Duration::from_millis(200));
        let started = Instant::now();
        let deposits = listener
            .fetch_deposits(&[SLOW.to_string(), FAST.to_string()])
            .await
            .unwrap();

        assert!(started.elapsed() < Duration::from_secs(2), "the slow address must time out");
        assert_eq!(deposits.len(), 1, "the other wallet is still polled");
        assert_eq!(deposits[0].tx_hash, "fast-tx");
    // --------------------------------------------------------------------------
    // #1045 — backoff duration calculation: 30 * 2^n capped at MAX_BACKOFF
    //
    // The existing backoff tests only assert on consecutive_404s count.
    // These tests check the actual computed Duration stored in next_poll_at,
    // verifying the doubling sequence and the MAX_BACKOFF ceiling.
    // --------------------------------------------------------------------------

    /// Helper: compute the backoff duration the listener will use after `n`
    /// consecutive 404s.  Mirrors the formula in `record_unfunded` exactly.
    fn expected_backoff(n: u32) -> Duration {
        let secs = 30u64
            .saturating_mul(2u64.saturating_pow(n.saturating_sub(1).min(10)));
        Duration::from_secs(secs).min(MAX_BACKOFF)
    }

    #[test]
    fn backoff_duration_after_1_consecutive_404_is_30s() {
        let listener = StellarListener::new("https://horizon-testnet.stellar.org".into());
        let addr = "GBACKOFF...1";

        let before = std::time::Instant::now();
        listener.record_unfunded(addr);
        let after = std::time::Instant::now();

        let map = listener.unfunded_backoff.lock().unwrap();
        let state = map.get(addr).unwrap();

        // next_poll_at should be approximately now + 30s.
        let expected = expected_backoff(1); // 30s
        assert_eq!(expected, Duration::from_secs(30));

        // Allow a small margin for the time between before/after.
        let lower = before + expected;
        let upper = after + expected + Duration::from_millis(100);
        assert!(
            state.next_poll_at >= lower && state.next_poll_at <= upper,
            "after 1 x 404, backoff should be 30s; next_poll_at={:?} expected in [{lower:?}, {upper:?}]",
            state.next_poll_at
        );
    }

    #[test]
    fn backoff_duration_after_2_consecutive_404s_is_60s() {
        let listener = StellarListener::new("https://horizon-testnet.stellar.org".into());
        let addr = "GBACKOFF...2";

        listener.record_unfunded(addr); // 1st → 30s
        let before = std::time::Instant::now();
        listener.record_unfunded(addr); // 2nd → 60s
        let after = std::time::Instant::now();

        let map = listener.unfunded_backoff.lock().unwrap();
        let state = map.get(addr).unwrap();

        let expected = expected_backoff(2); // 60s
        assert_eq!(expected, Duration::from_secs(60));

        let lower = before + expected;
        let upper = after + expected + Duration::from_millis(100);
        assert!(
            state.next_poll_at >= lower && state.next_poll_at <= upper,
            "after 2 x 404, backoff should be 60s; next_poll_at={:?} expected in [{lower:?}, {upper:?}]",
            state.next_poll_at
        );
    }

    #[test]
    fn backoff_duration_after_3_consecutive_404s_is_120s() {
        let listener = StellarListener::new("https://horizon-testnet.stellar.org".into());
        let addr = "GBACKOFF...3";

        for _ in 0..3 {
            listener.record_unfunded(addr);
        }

        let map = listener.unfunded_backoff.lock().unwrap();
        let state = map.get(addr).unwrap();
        assert_eq!(state.consecutive_404s, 3);

        let expected = expected_backoff(3); // 120s
        assert_eq!(expected, Duration::from_secs(120));
        // Verify the stored next_poll_at is at least now + 100s (generous lower
        // bound that won't flake — we just need the ceiling isn't premature).
        assert!(
            state.next_poll_at > std::time::Instant::now() + Duration::from_secs(100),
            "after 3 x 404, backoff should be ≥120s"
        );
    }

    #[test]
    fn backoff_duration_capped_at_max_backoff_after_10_consecutive_404s() {
        let listener = StellarListener::new("https://horizon-testnet.stellar.org".into());
        let addr = "GBACKOFF...10";

        // Drive past the cap: 10 consecutive 404s.
        for _ in 0..10 {
            listener.record_unfunded(addr);
        }

        let before = std::time::Instant::now();
        // One more — this is the 11th, which would be 30 * 2^10 = 30720s
        // without the cap.  The cap is MAX_BACKOFF = 600s.
        listener.record_unfunded(addr);
        let after = std::time::Instant::now();

        let map = listener.unfunded_backoff.lock().unwrap();
        let state = map.get(addr).unwrap();
        assert_eq!(state.consecutive_404s, 11);

        // Regardless of the raw doubling formula, the stored backoff must not
        // exceed MAX_BACKOFF (600s).
        let lower = before + MAX_BACKOFF;
        let upper = after + MAX_BACKOFF + Duration::from_millis(100);
        assert!(
            state.next_poll_at >= lower && state.next_poll_at <= upper,
            "backoff must be capped at {MAX_BACKOFF:?}; next_poll_at={:?} expected in [{lower:?}, {upper:?}]",
            state.next_poll_at
        );
    }

    #[test]
    fn backoff_formula_never_exceeds_max_backoff_for_any_n() {
        // Verify the pure formula independently of Instant arithmetic.
        for n in 1u32..=30 {
            let b = expected_backoff(n);
            assert!(
                b <= MAX_BACKOFF,
                "expected_backoff({n}) = {b:?} exceeds MAX_BACKOFF ({MAX_BACKOFF:?})"
            );
        }
    }

    #[test]
    fn clear_backoff_resets_consecutive_404s_count_to_zero() {
        let listener = StellarListener::new("https://horizon-testnet.stellar.org".into());
        let addr = "GCLEAR...BACKOFF";

        // Build up some consecutive 404s.
        for _ in 0..5 {
            listener.record_unfunded(addr);
        }
        {
            let map = listener.unfunded_backoff.lock().unwrap();
            assert_eq!(map.get(addr).unwrap().consecutive_404s, 5);
        }

        // clear_backoff removes the entry entirely — consecutive count goes
        // back to 0 (entry absent ≡ count 0) and should_skip returns false.
        listener.clear_backoff(addr);

        {
            let map = listener.unfunded_backoff.lock().unwrap();
            assert!(
                map.get(addr).is_none(),
                "clear_backoff must remove the entry, resetting consecutive_404s to 0"
            );
        }
        assert!(
            !listener.should_skip(addr),
            "after clear_backoff the address must not be skipped"
        );
    }

    #[test]
    fn backoff_doubling_sequence_is_correct() {
        // Verify the first four backoff steps match the expected sequence:
        // 30s, 60s, 120s, 240s.
        let expected_sequence = [
            Duration::from_secs(30),
            Duration::from_secs(60),
            Duration::from_secs(120),
            Duration::from_secs(240),
        ];
        for (i, &expected) in expected_sequence.iter().enumerate() {
            let n = (i + 1) as u32;
            assert_eq!(
                expected_backoff(n),
                expected,
                "backoff after {n} consecutive 404s should be {expected:?}"
            );
        }
    }
}
