//! Horizon HTTP fetching: the response types for an account's payments feed
//! and [`fetch_for_address`], which polls it. Mapping each record to a
//! deposit is delegated to the pure functions in
//! [`deposit_parser`](super::deposit_parser).

use serde::Deserialize;

use super::deposit_parser::record_to_deposit;
use super::stellar::DetectedDeposit;

#[derive(Debug, Deserialize)]
pub(crate) struct PaymentsPage {
    #[serde(rename = "_embedded")]
    pub(crate) embedded: Embedded,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Embedded {
    pub(crate) records: Vec<OperationRecord>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OperationRecord {
    #[serde(rename = "type")]
    pub(crate) op_type: String,
    pub(crate) transaction_successful: bool,
    pub(crate) transaction_hash: String,
    /// Ledger sequence the operation was included in.  Horizon returns this
    /// directly on the operation record; it may also appear on the joined
    /// transaction, so both are consulted.
    #[serde(default)]
    pub(crate) ledger: Option<u32>,
    #[serde(default)]
    pub(crate) to: Option<String>,
    /// `path_payment_strict_send` uses `destination` instead of `to`.
    #[serde(default)]
    pub(crate) destination: Option<String>,
    #[serde(default)]
    pub(crate) account: Option<String>,
    #[serde(default)]
    pub(crate) amount: Option<String>,
    #[serde(default)]
    pub(crate) amount_sent: Option<String>,
    #[serde(default)]
    pub(crate) starting_balance: Option<String>,
    #[serde(default)]
    pub(crate) asset_type: Option<String>,
    #[serde(default)]
    pub(crate) asset_code: Option<String>,
    /// `claimable_balance_created` uses `asset` (the full asset string) rather
    /// than `asset_type` / `asset_code`.
    #[serde(default)]
    pub(crate) asset: Option<String>,
    /// `claimable_balance_created` uses `amount` for the total amount.
    #[serde(default)]
    pub(crate) claimant: Option<ClaimantInfo>,
    #[serde(default)]
    pub(crate) transaction: Option<EmbeddedTransaction>,
}

/// Horizon wraps each claimant in the `claimants` array.  For the
/// `claimable_balance_created` operation the per-claimant `destination`
/// is the wallet the balance is claimable by.
#[derive(Debug, Deserialize)]
pub(crate) struct ClaimantInfo {
    pub(crate) destination: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EmbeddedTransaction {
    #[serde(default)]
    pub(crate) memo: Option<String>,
    /// Ledger sequence of the parent transaction, when Horizon includes it on
    /// the joined transaction rather than the operation record itself.
    #[serde(default)]
    pub(crate) ledger: Option<u32>,
}

/// Distinguishes a funded account (200 OK) from an unfunded one (404) so the
/// caller can apply backoff for the latter.
pub(crate) enum FetchResult {
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
pub(crate) async fn fetch_for_address(
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
