use async_trait::async_trait;
use serde::Deserialize;

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

/// One wallet address to poll, along with the paging cursor of the last
/// operation already processed for it (`None` on a wallet's first poll).
#[derive(Debug, Clone)]
pub struct AddressCursor {
    pub address: String,
    pub cursor: Option<String>,
}

/// Result of polling a single address: the deposits found and the cursor to
/// persist so the next poll doesn't re-fetch them.
pub struct AddressPollResult {
    pub address: String,
    pub deposits: Vec<DetectedDeposit>,
    pub next_cursor: Option<String>,
}

#[async_trait]
pub trait BlockchainListener: Send + Sync {
    async fn fetch_deposits(&self, addresses: &[AddressCursor]) -> Result<Vec<AddressPollResult>, String>;
}

pub struct StellarListener {
    pub horizon_url: String,
}

#[async_trait]
impl BlockchainListener for StellarListener {
    async fn fetch_deposits(&self, addresses: &[AddressCursor]) -> Result<Vec<AddressPollResult>, String> {
        let mut results = Vec::new();
        for entry in addresses {
            match fetch_for_address(&self.horizon_url, &entry.address, entry.cursor.as_deref()).await {
                Ok((deposits, next_cursor)) => results.push(AddressPollResult {
                    address: entry.address.clone(),
                    deposits,
                    next_cursor: next_cursor.or_else(|| entry.cursor.clone()),
                }),
                Err(err) => {
                    // One bad/unreachable address must not block deposit detection
                    // for every other wallet in this poll cycle.
                    tracing::warn!(error = %err, address = %entry.address, "failed to fetch deposits for address");
                }
            }
        }
        Ok(results)
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
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    starting_balance: Option<String>,
    #[serde(default)]
    asset_type: Option<String>,
    #[serde(default)]
    asset_code: Option<String>,
    #[serde(default)]
    transaction: Option<EmbeddedTransaction>,
    paging_token: String,
}

#[derive(Debug, Deserialize)]
struct EmbeddedTransaction {
    #[serde(default)]
    memo: Option<String>,
}

/// Polls Horizon's per-account payments feed for one wallet address and maps
/// successful incoming operations to deposits. A brand-new wallet's very
/// first funding always arrives as a `create_account` operation (Stellar
/// rejects `payment` ops to accounts that don't exist on-ledger yet), so both
/// operation types are handled here, not just `payment`.
///
/// When `cursor` is `None` (a wallet's first poll) the most recent 20
/// operations are fetched, matching the previous behavior. Once a cursor is
/// known, only operations after it are fetched (ascending order), so
/// already-processed payments are never re-fetched.
async fn fetch_for_address(
    horizon_url: &str,
    address: &str,
    cursor: Option<&str>,
) -> Result<(Vec<DetectedDeposit>, Option<String>), String> {
    let url = match cursor {
        Some(cursor) => format!(
            "{}/accounts/{address}/payments?order=asc&cursor={cursor}&limit=20&include_failed=false&join=transactions",
            horizon_url.trim_end_matches('/')
        ),
        None => format!(
            "{}/accounts/{address}/payments?order=desc&limit=20&include_failed=false&join=transactions",
            horizon_url.trim_end_matches('/')
        ),
    };

    let response = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // Account has no ledger history yet (never funded) — nothing to detect.
        return Ok((vec![], None));
    }

    let page: PaymentsPage = response
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    // The newest record's paging token becomes the next cursor: on the first
    // poll (order=desc) that's the first record; once polling ascending from
    // a cursor, it's the last one.
    let next_cursor = match cursor {
        Some(_) => page.embedded.records.last().map(|r| r.paging_token.clone()),
        None => page.embedded.records.first().map(|r| r.paging_token.clone()),
    };

    let mut deposits = Vec::new();
    for record in page.embedded.records {
        if !record.transaction_successful {
            continue;
        }

        let amount_str: Option<&str> = match record.op_type.as_str() {
            "create_account" if record.account.as_deref() == Some(address) => {
                record.starting_balance.as_deref()
            }
            "payment" | "path_payment_strict_receive" | "path_payment_strict_send"
                if record.to.as_deref() == Some(address) =>
            {
                record.amount.as_deref()
            }
            _ => None,
        };
        let Some(amount_str) = amount_str else { continue };

        let amount_stroops = match parse_amount_to_stroops(amount_str) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    tx_hash = %record.transaction_hash,
                    "skipping deposit with unparseable amount"
                );
                continue;
            }
        };

        let asset = match record.asset_type.as_deref() {
            Some("native") | None => "XLM".to_string(),
            _ => record.asset_code.clone().unwrap_or_else(|| "unknown".into()),
        };

        let memo = record.transaction.as_ref().and_then(|t| t.memo.clone());

        deposits.push(DetectedDeposit {
            tx_hash: record.transaction_hash,
            destination: address.to_string(),
            amount_stroops,
            asset,
            confirmations: 1,
            memo,
        });
    }
    Ok((deposits, next_cursor))
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
    use super::parse_amount_to_stroops;

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
}
