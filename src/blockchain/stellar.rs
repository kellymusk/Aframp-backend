use async_trait::async_trait;
use futures::{stream, StreamExt};
use serde::Deserialize;
use std::future::Future;

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
    pub poll_concurrency: usize,
}

#[async_trait]
impl BlockchainListener for StellarListener {
    async fn fetch_deposits(&self, addresses: &[String]) -> Result<Vec<DetectedDeposit>, String> {
        let horizon_url = self.horizon_url.clone();
        // Scaling: N wallets × average Horizon latency / concurrency = cycle
        // time. At 1,000 wallets, 300 ms, and concurrency 20, that is about
        // 15 seconds instead of roughly 300 seconds sequentially.
        Ok(
            fetch_deposits_bounded(addresses, self.poll_concurrency, move |address| {
                let horizon_url = horizon_url.clone();
                async move { fetch_for_address(&horizon_url, &address).await }
            })
            .await,
        )
    }
}

async fn fetch_deposits_bounded<F, Fut>(
    addresses: &[String],
    concurrency: usize,
    fetch: F,
) -> Vec<DetectedDeposit>
where
    F: Fn(String) -> Fut + Clone,
    Fut: Future<Output = Result<Vec<DetectedDeposit>, String>>,
{
    stream::iter(addresses.iter().cloned())
        .map(|address| {
            let fetch = fetch.clone();
            async move {
                match fetch(address.clone()).await {
                    Ok(found) => found,
                    Err(err) => {
                        // One bad/unreachable address must not block deposit detection
                        // for every other wallet in this poll cycle.
                        tracing::warn!(error = %err, %address, "failed to fetch deposits for address");
                        Vec::new()
                    }
                }
            }
        })
        .buffer_unordered(concurrency.max(1))
        .fold(Vec::new(), |mut deposits, mut found| async move {
            deposits.append(&mut found);
            deposits
        })
        .await
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
async fn fetch_for_address(
    horizon_url: &str,
    address: &str,
) -> Result<Vec<DetectedDeposit>, String> {
    let url = format!(
        "{}/accounts/{address}/payments?order=desc&limit=20&include_failed=false&join=transactions",
        horizon_url.trim_end_matches('/')
    );

    let response = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // Account has no ledger history yet (never funded) — nothing to detect.
        return Ok(vec![]);
    }

    let page: PaymentsPage = response
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

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
        let Some(amount_str) = amount_str else {
            continue;
        };

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
            _ => record
                .asset_code
                .clone()
                .unwrap_or_else(|| "unknown".into()),
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
    Ok(deposits)
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
    let whole_stroops: i64 = whole
        .parse()
        .map_err(|_| format!("invalid amount: {amount}"))?;
    let frac_stroops: i64 = frac_padded
        .parse()
        .map_err(|_| format!("invalid amount: {amount}"))?;
    Ok(whole_stroops * 10_000_000 + frac_stroops)
}

#[cfg(test)]
mod tests {
    use super::{fetch_deposits_bounded, parse_amount_to_stroops};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn parses_whole_and_fractional_amounts() {
        assert_eq!(
            parse_amount_to_stroops("10000.0000000").unwrap(),
            100_000_000_000
        );
        assert_eq!(parse_amount_to_stroops("0.5").unwrap(), 5_000_000);
        assert_eq!(parse_amount_to_stroops("1").unwrap(), 10_000_000);
        assert_eq!(
            parse_amount_to_stroops("123.4567890").unwrap(),
            1_234_567_890
        );
    }

    #[test]
    fn rejects_excess_precision() {
        assert!(parse_amount_to_stroops("1.12345678").is_err());
    }

    #[tokio::test]
    async fn wallet_fetches_respect_the_concurrency_limit() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let addresses = (0..40).map(|i| format!("wallet-{i}")).collect::<Vec<_>>();
        let limit = 4;

        fetch_deposits_bounded(&addresses, limit, {
            let active = active.clone();
            let maximum = maximum.clone();
            move |_address| {
                let active = active.clone();
                let maximum = maximum.clone();
                async move {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(Vec::new())
                }
            }
        })
        .await;

        assert!(maximum.load(Ordering::SeqCst) > 1);
        assert!(maximum.load(Ordering::SeqCst) <= limit);
    }
}
