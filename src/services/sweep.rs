use sqlx::PgPool;
use stellar_strkey::ed25519::PrivateKey;

use crate::blockchain::wallet_crypto;
use crate::services::wallets;

#[derive(Debug, thiserror::Error)]
pub enum SweepError {
    #[error("failed to decrypt wallet secret: {0}")]
    DecryptionFailed(String),
    #[error("failed to parse secret key: {0}")]
    InvalidSecretKey(String),
    #[error("failed to build transaction: {0}")]
    TransactionBuildFailed(String),
    #[error("failed to submit transaction to Horizon: {0}")]
    SubmissionFailed(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Represents a Stellar payment transaction ready for submission.
pub struct SweepTransaction {
    pub source_address: String,
    pub destination_address: String,
    pub amount_stroops: i64,
    pub asset: String,
    pub xdr: String,
}

/// Sweep funds from all merchant wallets to the platform wallet.
/// Returns the list of successfully submitted transactions.
pub async fn sweep_all_wallets(
    db: &PgPool,
    horizon_url: &str,
    platform_wallet: &str,
    encryption_key: &[u8; 32],
    min_balance_stroops: i64,
) -> Result<Vec<SweepTransaction>, SweepError> {
    let wallets = wallets::all_wallets(db).await?;
    let mut swept = Vec::new();

    for wallet in wallets {
        match sweep_wallet(
            horizon_url,
            &wallet.address,
            &wallet.encrypted_secret_key,
            platform_wallet,
            encryption_key,
            min_balance_stroops,
        )
        .await
        {
            Ok(Some(tx)) => swept.push(tx),
            Ok(None) => {
                tracing::debug!(address = %wallet.address, "wallet balance below minimum, skipping");
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    address = %wallet.address,
                    "failed to sweep wallet"
                );
            }
        }
    }

    Ok(swept)
}

/// Sweep a single wallet to the platform wallet.
/// Returns None if balance is below minimum threshold.
async fn sweep_wallet(
    horizon_url: &str,
    source_address: &str,
    encrypted_secret: &str,
    destination: &str,
    encryption_key: &[u8; 32],
    min_balance_stroops: i64,
) -> Result<Option<SweepTransaction>, SweepError> {
    // Decrypt the secret key
    let secret_seed = wallet_crypto::decrypt(encryption_key, encrypted_secret)
        .map_err(SweepError::DecryptionFailed)?;

    // Parse the secret key
    let _private_key = PrivateKey::from_string(&secret_seed)
        .map_err(|e| SweepError::InvalidSecretKey(e.to_string()))?;

    // Fetch account details from Horizon to get balance and sequence
    let account_url = format!(
        "{}/accounts/{}",
        horizon_url.trim_end_matches('/'),
        source_address
    );

    let response = reqwest::get(&account_url)
        .await
        .map_err(|e| SweepError::TransactionBuildFailed(e.to_string()))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // Account not yet funded on-chain
        return Ok(None);
    }

    let account: serde_json::Value = response
        .error_for_status()
        .map_err(|e| SweepError::TransactionBuildFailed(e.to_string()))?
        .json()
        .await
        .map_err(|e| SweepError::TransactionBuildFailed(e.to_string()))?;

    // Find XLM balance (native asset)
    let balances = account["balances"]
        .as_array()
        .ok_or_else(|| SweepError::TransactionBuildFailed("no balances array".into()))?;

    let xlm_balance = balances
        .iter()
        .find(|b| b["asset_type"].as_str() == Some("native"))
        .and_then(|b| b["balance"].as_str())
        .ok_or_else(|| SweepError::TransactionBuildFailed("no XLM balance found".into()))?;

    // Parse balance to stroops
    let balance_stroops = parse_stellar_amount(xlm_balance)?;

    // Calculate amount to sweep (leave minimum reserve)
    const BASE_RESERVE_STROOPS: i64 = 10_000_000; // 1 XLM minimum reserve
    const TRANSACTION_FEE_STROOPS: i64 = 100_000; // 0.01 XLM fee buffer

    let sweep_amount = balance_stroops
        .saturating_sub(BASE_RESERVE_STROOPS)
        .saturating_sub(TRANSACTION_FEE_STROOPS);

    if sweep_amount < min_balance_stroops {
        return Ok(None);
    }

    // TODO: Build and sign actual Stellar XDR transaction
    // For now, return placeholder - this needs stellar-base integration
    // to construct proper Payment operation XDR
    tracing::info!(
        source = %source_address,
        destination = %destination,
        amount = sweep_amount,
        "would sweep wallet (XDR construction not yet implemented)"
    );

    Ok(Some(SweepTransaction {
        source_address: source_address.to_string(),
        destination_address: destination.to_string(),
        amount_stroops: sweep_amount,
        asset: "XLM".to_string(),
        xdr: "TODO: construct and sign transaction XDR".to_string(),
    }))
}

fn parse_stellar_amount(amount: &str) -> Result<i64, SweepError> {
    let mut parts = amount.splitn(2, '.');
    let whole = parts.next().unwrap_or("0");
    let frac = parts.next().unwrap_or("");
    if frac.len() > 7 {
        return Err(SweepError::TransactionBuildFailed(format!(
            "unexpected precision in amount: {amount}"
        )));
    }
    let frac_padded = format!("{frac:0<7}");
    let whole_stroops: i64 = whole
        .parse()
        .map_err(|_| SweepError::TransactionBuildFailed(format!("invalid amount: {amount}")))?;
    let frac_stroops: i64 = frac_padded
        .parse()
        .map_err(|_| SweepError::TransactionBuildFailed(format!("invalid amount: {amount}")))?;
    Ok(whole_stroops * 10_000_000 + frac_stroops)
}
