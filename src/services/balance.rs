use crate::cache::{cache::Cache, keys::wallet::BalanceKey, RedisCache};
use crate::chains::stellar::{client::StellarClient, errors::StellarError, types::AssetBalance};
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::time::Duration;
use tracing::{debug, warn};

const BALANCE_CACHE_TTL: Duration = Duration::from_secs(30);
const BASE_RESERVE_XLM: &str = "1.0";
const TRUSTLINE_RESERVE_XLM: &str = "0.5";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletBalance {
    pub wallet_address: String,
    pub chain: String,
    pub balances: BalanceDetails,
    pub trustlines: Vec<TrustlineInfo>,
    pub minimum_xlm_required: String,
    pub last_updated: String,
    pub cached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceDetails {
    pub xlm: XlmBalance,
    pub cngn: CngnBalance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XlmBalance {
    pub total: String,
    pub available: String,
    pub reserved: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CngnBalance {
    pub balance: String,
    pub trustline_exists: bool,
    pub issuer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustlineInfo {
    pub asset_code: String,
    pub asset_issuer: String,
    pub balance: String,
    pub limit: String,
}

pub struct BalanceService {
    stellar_client: StellarClient,
    cache: RedisCache,
    cngn_issuer: String,
}

impl BalanceService {
    pub fn new(stellar_client: StellarClient, cache: RedisCache, cngn_issuer: String) -> Self {
        Self {
            stellar_client,
            cache,
            cngn_issuer,
        }
    }

    pub async fn get_balance(
        &self,
        address: &str,
        force_refresh: bool,
    ) -> Result<WalletBalance, StellarError> {
        let cache_key = BalanceKey::new(address).to_string();

        if !force_refresh {
            if let Ok(Some(cached)) = self.cache.get(&cache_key).await {
                debug!("Balance cache hit for {}", address);
                return Ok(cached);
            }
        }

        debug!("Fetching balance from Stellar for {}", address);
        let account = self.stellar_client.get_account(address).await?;

        let xlm_balance = self.extract_xlm_balance(&account.balances);
        let trustline_count = account
            .balances
            .iter()
            .filter(|b| {
                matches!(
                    b.asset_type.as_str(),
                    "credit_alphanum4" | "credit_alphanum12"
                )
            })
            .count();

        let reserved = self.calculate_reserve(trustline_count);
        let available = self.calculate_available(&xlm_balance, &reserved);

        let cngn_balance = self.extract_cngn_balance(&account.balances);
        let trustlines = self.extract_trustlines(&account.balances);

        let balance = WalletBalance {
            wallet_address: address.to_string(),
            chain: "stellar".to_string(),
            balances: BalanceDetails {
                xlm: XlmBalance {
                    total: xlm_balance,
                    available,
                    reserved,
                },
                cngn: cngn_balance,
            },
            trustlines,
            minimum_xlm_required: self.calculate_reserve(trustline_count),
            last_updated: Utc::now().to_rfc3339(),
            cached: false,
        };

        if let Err(e) = self
            .cache
            .set(&cache_key, &balance, Some(BALANCE_CACHE_TTL))
            .await
        {
            warn!("Failed to cache balance for {}: {}", address, e);
        }

        Ok(balance)
    }

    fn extract_xlm_balance(&self, balances: &[AssetBalance]) -> String {
        balances
            .iter()
            .find(|b| b.asset_type == "native")
            .map(|b| b.balance.clone())
            .unwrap_or_else(|| "0.0000000".to_string())
    }

    fn extract_cngn_balance(&self, balances: &[AssetBalance]) -> CngnBalance {
        let cngn = balances.iter().find(|b| {
            matches!(
                b.asset_type.as_str(),
                "credit_alphanum4" | "credit_alphanum12"
            ) && b.asset_code.as_deref() == Some("cNGN")
                && b.asset_issuer.as_deref() == Some(&self.cngn_issuer)
        });

        match cngn {
            Some(balance) => CngnBalance {
                balance: balance.balance.clone(),
                trustline_exists: true,
                issuer: Some(self.cngn_issuer.clone()),
            },
            None => CngnBalance {
                balance: "0.00".to_string(),
                trustline_exists: false,
                issuer: None,
            },
        }
    }

    fn extract_trustlines(&self, balances: &[AssetBalance]) -> Vec<TrustlineInfo> {
        balances
            .iter()
            .filter(|b| {
                matches!(
                    b.asset_type.as_str(),
                    "credit_alphanum4" | "credit_alphanum12"
                )
            })
            .filter_map(|b| {
                Some(TrustlineInfo {
                    asset_code: b.asset_code.clone()?,
                    asset_issuer: b.asset_issuer.clone()?,
                    balance: b.balance.clone(),
                    limit: b.limit.clone().unwrap_or_else(|| "unlimited".to_string()),
                })
            })
            .collect()
    }

    fn calculate_reserve(&self, trustline_count: usize) -> String {
        let base = Decimal::from_str(BASE_RESERVE_XLM).unwrap();
        let per_trustline = Decimal::from_str(TRUSTLINE_RESERVE_XLM).unwrap();
        let total = base + (per_trustline * Decimal::from(trustline_count));
        format!("{:.7}", total)
    }

    fn calculate_available(&self, total: &str, reserved: &str) -> String {
        let total_dec = Decimal::from_str(total).unwrap_or(Decimal::ZERO);
        let reserved_dec = Decimal::from_str(reserved).unwrap_or(Decimal::ZERO);
        let available = (total_dec - reserved_dec).max(Decimal::ZERO);
        format!("{:.7}", available)
    }
}
