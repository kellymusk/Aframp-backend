use crate::database::error::{DatabaseError, DatabaseErrorKind};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

type BigDecimal = sqlx::types::BigDecimal;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeBreakdown {
    #[serde(with = "bigdecimal_serde")]
    pub amount: BigDecimal,
    pub currency: String,
    pub provider: Option<ProviderFee>,
    pub platform: PlatformFee,
    pub stellar: StellarFee,
    #[serde(with = "bigdecimal_serde")]
    pub total: BigDecimal,
    #[serde(with = "bigdecimal_serde")]
    pub net_amount: BigDecimal,
    #[serde(with = "bigdecimal_serde")]
    pub effective_rate: BigDecimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderFee {
    pub name: String,
    pub method: String,
    #[serde(with = "bigdecimal_serde")]
    pub percent: BigDecimal,
    #[serde(with = "bigdecimal_serde")]
    pub flat: BigDecimal,
    #[serde(with = "bigdecimal_serde_opt")]
    pub cap: Option<BigDecimal>,
    #[serde(with = "bigdecimal_serde")]
    pub calculated: BigDecimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformFee {
    #[serde(with = "bigdecimal_serde")]
    pub percent: BigDecimal,
    #[serde(with = "bigdecimal_serde")]
    pub calculated: BigDecimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StellarFee {
    #[serde(with = "bigdecimal_serde")]
    pub xlm: BigDecimal,
    #[serde(with = "bigdecimal_serde")]
    pub ngn: BigDecimal,
    pub absorbed: bool,
}

mod bigdecimal_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use sqlx::types::BigDecimal;
    use std::str::FromStr;

    pub fn serialize<S>(value: &BigDecimal, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.to_string().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<BigDecimal, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        BigDecimal::from_str(&s).map_err(serde::de::Error::custom)
    }
}

mod bigdecimal_serde_opt {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use sqlx::types::BigDecimal;
    use std::str::FromStr;

    pub fn serialize<S>(value: &Option<BigDecimal>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(v) => serializer.serialize_some(&v.to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<BigDecimal>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt = Option::<String>::deserialize(deserializer)?;
        opt.map(|s| BigDecimal::from_str(&s).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[derive(Debug, Clone)]
struct FeeConfig {
    id: Uuid,
    transaction_type: String,
    payment_provider: Option<String>,
    payment_method: Option<String>,
    min_amount: Option<BigDecimal>,
    max_amount: Option<BigDecimal>,
    provider_fee_percent: Option<BigDecimal>,
    provider_fee_flat: Option<BigDecimal>,
    provider_fee_cap: Option<BigDecimal>,
    platform_fee_percent: Option<BigDecimal>,
}

pub struct FeeCalculationService {
    pool: PgPool,
    cache: Arc<RwLock<HashMap<String, Vec<FeeConfig>>>>,
    xlm_rate_cache: Arc<RwLock<Option<(BigDecimal, chrono::DateTime<chrono::Utc>)>>>,
}

impl FeeCalculationService {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            cache: Arc::new(RwLock::new(HashMap::new())),
            xlm_rate_cache: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn calculate_fees(
        &self,
        transaction_type: &str,
        amount: BigDecimal,
        provider: Option<&str>,
        payment_method: Option<&str>,
    ) -> Result<FeeBreakdown, DatabaseError> {
        let currency = "NGN".to_string();

        let fee_config = self
            .find_matching_tier(transaction_type, &amount, provider, payment_method)
            .await?;

        let provider_fee = if let Some(config) = &fee_config {
            self.calculate_provider_fee(&amount, config, provider, payment_method)
        } else {
            None
        };

        let platform_fee = if let Some(config) = &fee_config {
            self.calculate_platform_fee(&amount, config)
        } else {
            PlatformFee {
                percent: BigDecimal::from_str("0").unwrap(),
                calculated: BigDecimal::from_str("0").unwrap(),
            }
        };

        let stellar_fee = self.calculate_stellar_fee().await;

        let total = provider_fee
            .as_ref()
            .map(|p| p.calculated.clone())
            .unwrap_or_else(|| BigDecimal::from_str("0").unwrap())
            + platform_fee.calculated.clone()
            + stellar_fee.ngn.clone();

        let net_amount = &amount - &total;
        let effective_rate = if amount > BigDecimal::from_str("0").unwrap() {
            (&total / &amount) * BigDecimal::from_str("100").unwrap()
        } else {
            BigDecimal::from_str("0").unwrap()
        };

        let breakdown = FeeBreakdown {
            amount: amount.clone(),
            currency,
            provider: provider_fee,
            platform: platform_fee,
            stellar: stellar_fee,
            total: total.clone(),
            net_amount,
            effective_rate,
        };

        if let Some(config) = fee_config {
            self.log_calculation(&breakdown, config.id, transaction_type)
                .await?;
        }

        Ok(breakdown)
    }

    pub async fn estimate_fees(
        &self,
        transaction_type: &str,
        amount: BigDecimal,
    ) -> Result<(BigDecimal, BigDecimal), DatabaseError> {
        let providers = vec!["flutterwave", "paystack"];
        let mut min_fee = None;
        let mut max_fee = None;

        for provider in providers {
            let breakdown = self
                .calculate_fees(
                    transaction_type,
                    amount.clone(),
                    Some(provider),
                    Some("card"),
                )
                .await?;

            if min_fee.is_none() || breakdown.total < min_fee.clone().unwrap() {
                min_fee = Some(breakdown.total.clone());
            }
            if max_fee.is_none() || breakdown.total > max_fee.clone().unwrap() {
                max_fee = Some(breakdown.total.clone());
            }
        }

        Ok((
            min_fee.unwrap_or_else(|| BigDecimal::from_str("0").unwrap()),
            max_fee.unwrap_or_else(|| BigDecimal::from_str("0").unwrap()),
        ))
    }

    async fn find_matching_tier(
        &self,
        transaction_type: &str,
        amount: &BigDecimal,
        provider: Option<&str>,
        payment_method: Option<&str>,
    ) -> Result<Option<FeeConfig>, DatabaseError> {
        let cache_key = format!(
            "{}:{}:{}",
            transaction_type,
            provider.unwrap_or("default"),
            payment_method.unwrap_or("default")
        );

        {
            let cache = self.cache.read().await;
            if let Some(configs) = cache.get(&cache_key) {
                for config in configs {
                    if self.amount_in_range(amount, &config.min_amount, &config.max_amount) {
                        return Ok(Some(config.clone()));
                    }
                }
            }
        }

        let configs = self
            .load_fee_configs(transaction_type, provider, payment_method)
            .await?;

        {
            let mut cache = self.cache.write().await;
            cache.insert(cache_key, configs.clone());
        }

        for config in configs {
            if self.amount_in_range(amount, &config.min_amount, &config.max_amount) {
                return Ok(Some(config));
            }
        }

        Ok(None)
    }

    fn amount_in_range(
        &self,
        amount: &BigDecimal,
        min: &Option<BigDecimal>,
        max: &Option<BigDecimal>,
    ) -> bool {
        let above_min = min.as_ref().map(|m| amount >= m).unwrap_or(true);
        let below_max = max.as_ref().map(|m| amount <= m).unwrap_or(true);
        above_min && below_max
    }

    async fn load_fee_configs(
        &self,
        transaction_type: &str,
        provider: Option<&str>,
        payment_method: Option<&str>,
    ) -> Result<Vec<FeeConfig>, DatabaseError> {
        let query = r#"
            SELECT id, transaction_type, payment_provider, payment_method,
                   min_amount, max_amount, provider_fee_percent, provider_fee_flat,
                   provider_fee_cap, platform_fee_percent
            FROM fee_structures
            WHERE transaction_type = $1
              AND is_active = TRUE
              AND effective_from <= NOW()
              AND (effective_until IS NULL OR effective_until >= NOW())
              AND ($2::TEXT IS NULL OR payment_provider IS NULL OR payment_provider = $2)
              AND ($3::TEXT IS NULL OR payment_method IS NULL OR payment_method = $3)
            ORDER BY min_amount ASC NULLS FIRST
        "#;

        #[derive(sqlx::FromRow)]
        struct FeeConfigRow {
            id: Uuid,
            transaction_type: String,
            payment_provider: Option<String>,
            payment_method: Option<String>,
            min_amount: Option<BigDecimal>,
            max_amount: Option<BigDecimal>,
            provider_fee_percent: Option<BigDecimal>,
            provider_fee_flat: Option<BigDecimal>,
            provider_fee_cap: Option<BigDecimal>,
            platform_fee_percent: Option<BigDecimal>,
        }

        let rows = sqlx::query_as::<_, FeeConfigRow>(query)
            .bind(transaction_type)
            .bind(provider)
            .bind(payment_method)
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::from_sqlx)?;

        let configs = rows
            .into_iter()
            .map(|row| FeeConfig {
                id: row.id,
                transaction_type: row.transaction_type,
                payment_provider: row.payment_provider,
                payment_method: row.payment_method,
                min_amount: row.min_amount,
                max_amount: row.max_amount,
                provider_fee_percent: row.provider_fee_percent,
                provider_fee_flat: row.provider_fee_flat,
                provider_fee_cap: row.provider_fee_cap,
                platform_fee_percent: row.platform_fee_percent,
            })
            .collect();

        Ok(configs)
    }

    fn calculate_provider_fee(
        &self,
        amount: &BigDecimal,
        config: &FeeConfig,
        provider: Option<&str>,
        payment_method: Option<&str>,
    ) -> Option<ProviderFee> {
        let percent = config.provider_fee_percent.clone()?;
        let flat = config
            .provider_fee_flat
            .clone()
            .unwrap_or_else(|| BigDecimal::from_str("0").unwrap());

        let mut calculated = (amount * &percent / BigDecimal::from_str("100").unwrap()) + &flat;

        if let Some(cap) = &config.provider_fee_cap {
            if &calculated > cap {
                calculated = cap.clone();
            }
        }

        Some(ProviderFee {
            name: provider.unwrap_or("unknown").to_string(),
            method: payment_method.unwrap_or("unknown").to_string(),
            percent,
            flat,
            cap: config.provider_fee_cap.clone(),
            calculated,
        })
    }

    fn calculate_platform_fee(&self, amount: &BigDecimal, config: &FeeConfig) -> PlatformFee {
        let percent = config
            .platform_fee_percent
            .clone()
            .unwrap_or_else(|| BigDecimal::from_str("0").unwrap());
        let calculated = amount * &percent / BigDecimal::from_str("100").unwrap();

        PlatformFee {
            percent,
            calculated,
        }
    }

    async fn calculate_stellar_fee(&self) -> StellarFee {
        let xlm_fee = BigDecimal::from_str("0.00001").unwrap();
        let xlm_rate = self
            .get_xlm_rate()
            .await
            .unwrap_or_else(|| BigDecimal::from_str("150").unwrap());
        let _ngn_fee = &xlm_fee * &xlm_rate;

        StellarFee {
            xlm: xlm_fee,
            ngn: BigDecimal::from_str("0").unwrap(), // Absorbed by platform
            absorbed: true,
        }
    }

    async fn get_xlm_rate(&self) -> Option<BigDecimal> {
        let cache = self.xlm_rate_cache.read().await;
        if let Some((rate, timestamp)) = cache.as_ref() {
            if chrono::Utc::now()
                .signed_duration_since(*timestamp)
                .num_minutes()
                < 5
            {
                return Some(rate.clone());
            }
        }
        drop(cache);

        // In production, fetch from CoinGecko or similar
        // For now, return default
        let rate = BigDecimal::from_str("150").unwrap();
        let mut cache = self.xlm_rate_cache.write().await;
        *cache = Some((rate.clone(), chrono::Utc::now()));
        Some(rate)
    }

    async fn log_calculation(
        &self,
        breakdown: &FeeBreakdown,
        fee_structure_id: Uuid,
        transaction_type: &str,
    ) -> Result<(), DatabaseError> {
        let query = r#"
            INSERT INTO fee_calculation_logs 
            (transaction_type, amount, currency, payment_provider, payment_method,
             fee_structure_id, provider_fee, platform_fee, stellar_fee_xlm, 
             stellar_fee_ngn, total_fees, net_amount, effective_rate)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        "#;

        sqlx::query(query)
            .bind(transaction_type)
            .bind(&breakdown.amount)
            .bind(&breakdown.currency)
            .bind(breakdown.provider.as_ref().map(|p| p.name.as_str()))
            .bind(breakdown.provider.as_ref().map(|p| p.method.as_str()))
            .bind(fee_structure_id)
            .bind(
                breakdown
                    .provider
                    .as_ref()
                    .map(|p| &p.calculated)
                    .unwrap_or(&BigDecimal::from_str("0").unwrap()),
            )
            .bind(&breakdown.platform.calculated)
            .bind(&breakdown.stellar.xlm)
            .bind(&breakdown.stellar.ngn)
            .bind(&breakdown.total)
            .bind(&breakdown.net_amount)
            .bind(&breakdown.effective_rate)
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::from_sqlx)?;

        Ok(())
    }

    pub async fn invalidate_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
        info!("Fee calculation cache invalidated");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_amount_in_range() {
        let amount = BigDecimal::from_str("10000").unwrap();
        let min = Some(BigDecimal::from_str("1000").unwrap());
        let max = Some(BigDecimal::from_str("50000").unwrap());

        // Create a mock service just for testing the method
        let pool = PgPool::connect_lazy("postgresql://test").unwrap();
        let service = FeeCalculationService::new(pool);

        assert!(service.amount_in_range(&amount, &min, &max));
    }

    #[test]
    fn test_fee_breakdown_serialization() {
        let breakdown = FeeBreakdown {
            amount: BigDecimal::from_str("100000").unwrap(),
            currency: "NGN".to_string(),
            provider: Some(ProviderFee {
                name: "flutterwave".to_string(),
                method: "card".to_string(),
                percent: BigDecimal::from_str("1.4").unwrap(),
                flat: BigDecimal::from_str("0").unwrap(),
                cap: Some(BigDecimal::from_str("2000").unwrap()),
                calculated: BigDecimal::from_str("1400").unwrap(),
            }),
            platform: PlatformFee {
                percent: BigDecimal::from_str("0.3").unwrap(),
                calculated: BigDecimal::from_str("300").unwrap(),
            },
            stellar: StellarFee {
                xlm: BigDecimal::from_str("0.00001").unwrap(),
                ngn: BigDecimal::from_str("0").unwrap(),
                absorbed: true,
            },
            total: BigDecimal::from_str("1700").unwrap(),
            net_amount: BigDecimal::from_str("98300").unwrap(),
            effective_rate: BigDecimal::from_str("1.7").unwrap(),
        };

        let json = serde_json::to_string(&breakdown).unwrap();
        assert!(json.contains("flutterwave"));
    }
}
