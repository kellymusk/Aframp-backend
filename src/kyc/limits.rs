use bigdecimal::BigDecimal;
use chrono::{DateTime, NaiveDate, Utc};
use redis::AsyncCommands;
use std::collections::HashMap;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::cache::RedisManager;
use crate::database::error::DatabaseError;
use crate::database::kyc_repository::{KycLimits, KycRepository, KycTier};
use crate::kyc::tier_requirements::{
    KycTierRequirements, LimitViolation, TransactionLimitEnforcer,
};

#[derive(Clone)]
pub struct KycLimitsEnforcer {
    repository: KycRepository,
    redis: RedisManager,
}

impl KycLimitsEnforcer {
    pub fn new(repository: KycRepository, redis: RedisManager) -> Self {
        Self { repository, redis }
    }

    /// Check if a transaction is allowed based on KYC tier limits
    pub async fn check_transaction_limits(
        &self,
        consumer_id: Uuid,
        transaction_amount: BigDecimal,
    ) -> Result<TransactionLimitCheckResult, KycLimitsError> {
        info!(
            "Checking transaction limits for consumer {} amount {}",
            consumer_id, transaction_amount
        );

        // Get current KYC record
        let kyc_record = self
            .repository
            .get_kyc_record_by_consumer(consumer_id)
            .await?
            .ok_or(KycLimitsError::KycRecordNotFound)?;

        // Get current limits
        let current_limits = self
            .repository
            .get_current_limits(consumer_id)
            .await?
            .ok_or(KycLimitsError::LimitsNotFound)?;

        // Use effective tier (might be reduced during EDD)
        let enforcer = TransactionLimitEnforcer::new(kyc_record.effective_tier);

        let limit_result = enforcer.check_transaction_limits(
            transaction_amount.clone(),
            current_limits.daily_volume_used.clone(),
            current_limits.monthly_volume_used.clone(),
        );

        // Update volume trackers if transaction is allowed
        let updated_limits = if limit_result.is_allowed {
            match self
                .update_volume_trackers(consumer_id, transaction_amount.clone())
                .await
            {
                Ok(updated) => Some(updated),
                Err(e) => {
                    error!(
                        "Failed to update volume trackers for consumer {}: {}",
                        consumer_id, e
                    );
                    None
                }
            }
        } else {
            None
        };

        Ok(TransactionLimitCheckResult {
            is_allowed: limit_result.is_allowed,
            violations: limit_result.violations,
            current_tier: kyc_record.tier,
            effective_tier: kyc_record.effective_tier,
            transaction_amount,
            remaining_limits: crate::kyc::tier_requirements::RemainingLimits {
                single_transaction: limit_result.daily_remaining.clone(),
                daily_volume: limit_result.daily_remaining,
                monthly_volume: limit_result.monthly_remaining,
            },
            updated_limits,
        })
    }

    /// Get current transaction limits for a consumer
    pub async fn get_current_limits(
        &self,
        consumer_id: Uuid,
    ) -> Result<KycLimitsInfo, KycLimitsError> {
        info!("Getting current limits for consumer {}", consumer_id);

        // Get current KYC record
        let kyc_record = self
            .repository
            .get_kyc_record_by_consumer(consumer_id)
            .await?
            .ok_or(KycLimitsError::KycRecordNotFound)?;

        // Get tier definition
        let tier_def = KycTierRequirements::get_tier_definition(kyc_record.effective_tier);

        // Get current volumes
        let (daily_used, monthly_used) = self.get_current_volumes(consumer_id).await?;

        Ok(KycLimitsInfo {
            tier: kyc_record.tier,
            effective_tier: kyc_record.effective_tier,
            max_transaction_amount: tier_def.max_transaction_amount,
            daily_volume_limit: tier_def.daily_volume_limit,
            monthly_volume_limit: tier_def.monthly_volume_limit,
            daily_volume_used: daily_used.clone(),
            monthly_volume_used: monthly_used.clone(),
            daily_remaining: &tier_def.daily_volume_limit - &daily_used,
            monthly_remaining: &tier_def.monthly_volume_limit - &monthly_used,
            last_daily_reset: Utc::now().date_naive(),
            last_monthly_reset: Utc::now().date_naive(),
            enhanced_due_diligence_active: kyc_record.enhanced_due_diligence_active,
        })
    }

    /// Reduce consumer's effective tier (used during EDD)
    pub async fn reduce_effective_tier(
        &self,
        consumer_id: Uuid,
        reduced_tier: KycTier,
        reason: String,
    ) -> Result<(), KycLimitsError> {
        info!(
            "Reducing effective tier for consumer {} to {:?}: {}",
            consumer_id, reduced_tier, reason
        );

        let kyc_record = self
            .repository
            .get_kyc_record_by_consumer(consumer_id)
            .await?
            .ok_or(KycLimitsError::KycRecordNotFound)?;

        // Update effective tier
        sqlx::query!(
            r#"
            UPDATE kyc_records 
            SET effective_tier = $1, enhanced_due_diligence_active = true, updated_at = $2
            WHERE id = $3
            "#,
            reduced_tier as KycTier,
            Utc::now(),
            kyc_record.id
        )
        .execute(&self.repository.pool)
        .await
        .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;

        // Log the change
        self.repository
            .create_event(
                consumer_id,
                Some(kyc_record.id),
                crate::database::kyc_repository::KycEventType::LimitsUpdated,
                Some(format!(
                    "Effective tier reduced to {:?}: {}",
                    reduced_tier, reason
                )),
                None,
                Some(serde_json::json!({
                    "previous_tier": format!("{:?}", kyc_record.effective_tier),
                    "new_tier": format!("{:?}", reduced_tier),
                    "reason": reason
                })),
            )
            .await
            .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;

        // Clear cache
        self.clear_limits_cache(consumer_id).await?;

        Ok(())
    }

    /// Restore consumer's effective tier (used after EDD completion)
    pub async fn restore_effective_tier(
        &self,
        consumer_id: Uuid,
        reason: String,
    ) -> Result<(), KycLimitsError> {
        info!(
            "Restoring effective tier for consumer {}: {}",
            consumer_id, reason
        );

        let kyc_record = self
            .repository
            .get_kyc_record_by_consumer(consumer_id)
            .await?
            .ok_or(KycLimitsError::KycRecordNotFound)?;

        // Restore effective tier to match actual tier
        sqlx::query!(
            r#"
            UPDATE kyc_records 
            SET effective_tier = tier, enhanced_due_diligence_active = false, updated_at = $1
            WHERE id = $2
            "#,
            Utc::now(),
            kyc_record.id
        )
        .execute(&self.repository.pool)
        .await
        .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;

        // Log the change
        self.repository
            .create_event(
                consumer_id,
                Some(kyc_record.id),
                crate::database::kyc_repository::KycEventType::LimitsUpdated,
                Some(format!("Effective tier restored: {}", reason)),
                None,
                Some(serde_json::json!({
                    "restored_tier": format!("{:?}", kyc_record.tier),
                    "reason": reason
                })),
            )
            .await
            .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;

        // Clear cache
        self.clear_limits_cache(consumer_id).await?;

        Ok(())
    }

    /// Reset daily volume counters (called by scheduled job)
    pub async fn reset_daily_counters(&self) -> Result<usize, KycLimitsError> {
        info!("Resetting daily volume counters");

        let today = Utc::now().date_naive();

        let result = sqlx::query!(
            r#"
            UPDATE kyc_volume_trackers 
            SET daily_volume = 0, transaction_count = 0, last_updated = $1
            WHERE date = $2 AND daily_volume > 0
            "#,
            Utc::now(),
            today
        )
        .execute(&self.repository.pool)
        .await
        .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;

        let count = result.rows_affected();
        info!("Reset daily counters for {} consumers", count);

        // Clear all limits cache
        self.clear_all_limits_cache().await?;

        Ok(count as usize)
    }

    /// Reset monthly volume counters (called by scheduled job)
    pub async fn reset_monthly_counters(&self) -> Result<usize, KycLimitsError> {
        info!("Resetting monthly volume counters");

        let today = Utc::now().date_naive();

        let result = sqlx::query!(
            r#"
            UPDATE kyc_volume_trackers 
            SET monthly_volume = 0, last_updated = $1
            WHERE date = $2 AND monthly_volume > 0
            "#,
            Utc::now(),
            today
        )
        .execute(&self.repository.pool)
        .await
        .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;

        let count = result.rows_affected();
        info!("Reset monthly counters for {} consumers", count);

        // Clear all limits cache
        self.clear_all_limits_cache().await?;

        Ok(count as usize)
    }

    /// Get consumers approaching their limits (for monitoring)
    pub async fn get_consumers_approaching_limits(
        &self,
        daily_threshold: f64,   // e.g., 0.8 for 80%
        monthly_threshold: f64, // e.g., 0.8 for 80%
        limit: Option<i64>,
    ) -> Result<Vec<ConsumerLimitWarning>, KycLimitsError> {
        info!("Getting consumers approaching limits");

        let limit = limit.unwrap_or(100);

        let records = sqlx::query(
            r#"
            SELECT 
                kr.consumer_id,
                kr.tier,
                kr.effective_tier,
                tdl.daily_volume_limit,
                tdl.monthly_volume_limit,
                COALESCE(vt.daily_volume, '0'::numeric) as daily_volume,
                COALESCE(vt.monthly_volume, '0'::numeric) as monthly_volume
            FROM kyc_records kr
            JOIN kyc_tier_definitions tdl ON kr.effective_tier = tdl.tier
            LEFT JOIN kyc_volume_trackers vt ON kr.consumer_id = vt.consumer_id AND vt.date = CURRENT_DATE
            WHERE kr.status = 'approved'
            AND (
                (COALESCE(vt.daily_volume, '0'::numeric) / tdl.daily_volume_limit) > $1
                OR (COALESCE(vt.monthly_volume, '0'::numeric) / tdl.monthly_volume_limit) > $2
            )
            ORDER BY (COALESCE(vt.daily_volume, '0'::numeric) / tdl.daily_volume_limit) DESC
            LIMIT $3
            "#
        )
        .bind(daily_threshold)
        .bind(monthly_threshold)
        .bind(limit)
        .fetch_all(&self.repository.pool)
        .await
        .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;

        let warnings: Vec<ConsumerLimitWarning> = Vec::new();
        for record in records {
            use sqlx::Row;
            let consumer_id: Uuid = record.try_get("consumer_id")
                .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;
            let tier: KycTier = record.try_get("tier")
                .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;
            let effective_tier: KycTier = record.try_get("effective_tier")
                .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;
            let daily_volume_limit: BigDecimal = record.try_get("daily_volume_limit")
                .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;
            let monthly_volume_limit: BigDecimal = record.try_get("monthly_volume_limit")
                .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;
            let daily_volume: BigDecimal = record.try_get("daily_volume")
                .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;
            let monthly_volume: BigDecimal = record.try_get("monthly_volume")
                .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;

            warnings.push(ConsumerLimitWarning {
                consumer_id,
                tier,
                effective_tier,
                daily_volume_limit,
                monthly_volume_limit,
                daily_volume_used: daily_volume,
                monthly_volume_used: monthly_volume,
            });
        }

        let result: Vec<ConsumerLimitWarning> = warnings
            .into_iter()
            .map(|warning| {
                let daily_usage_ratio =
                    &warning.daily_volume_used / &warning.daily_volume_limit;
                let monthly_usage_ratio =
                    &warning.monthly_volume_used / &warning.monthly_volume_limit;

                ConsumerLimitWarning {
                    consumer_id: warning.consumer_id,
                    tier: warning.tier,
                    effective_tier: warning.effective_tier,
                    daily_usage_ratio: daily_usage_ratio.to_f64().unwrap_or(0.0),
                    monthly_usage_ratio: monthly_usage_ratio.to_f64().unwrap_or(0.0),
                    daily_volume_used: warning.daily_volume_used,
                    monthly_volume_used: warning.monthly_volume_used,
                    daily_volume_limit: warning.daily_volume_limit,
                    monthly_volume_limit: warning.monthly_volume_limit,
                }
            })
            .collect();

        Ok(warnings)
    }

    async fn update_volume_trackers(
        &self,
        consumer_id: Uuid,
        transaction_amount: BigDecimal,
    ) -> Result<KycLimits, KycLimitsError> {
        let updated_tracker = self
            .repository
            .update_volume_tracker(consumer_id, transaction_amount.clone())
            .await
            .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;

        // Get updated limits
        let limits = self
            .repository
            .get_current_limits(consumer_id)
            .await?
            .ok_or(KycLimitsError::LimitsNotFound)?;

        // Update cache
        self.update_limits_cache(consumer_id, &limits).await?;

        Ok(limits)
    }

    async fn get_current_volumes(
        &self,
        consumer_id: Uuid,
    ) -> Result<(BigDecimal, BigDecimal), KycLimitsError> {
        let cache_key = format!("kyc_volumes:{}", consumer_id);

        // Try cache first
        if let Ok(cached_data) = self.redis.get(&cache_key).await {
            if let Ok(volumes) = serde_json::from_str::<serde_json::Value>(&cached_data) {
                let daily = BigDecimal::from_str(volumes["daily"].as_str().unwrap_or("0"))
                    .unwrap_or_default();
                let monthly = BigDecimal::from_str(volumes["monthly"].as_str().unwrap_or("0"))
                    .unwrap_or_default();
                return Ok((daily, monthly));
            }
        }

        // Fallback to database
        let today = Utc::now().date_naive();

        let result = sqlx::query!(
            r#"
            SELECT COALESCE(daily_volume, '0'::numeric) as daily_volume,
                   COALESCE(monthly_volume, '0'::numeric) as monthly_volume
            FROM kyc_volume_trackers
            WHERE consumer_id = $1 AND date = $2
            "#,
            consumer_id,
            today
        )
        .fetch_optional(&self.repository.pool)
        .await
        .map_err(|e| KycLimitsError::DatabaseError(e.to_string()))?;

        let (daily, monthly) = match result {
            Some(record) => (record.daily_volume, record.monthly_volume),
            None => (BigDecimal::from(0), BigDecimal::from(0)),
        };

        // Update cache
        let volumes_data = serde_json::json!({
            "daily": daily.to_string(),
            "monthly": monthly.to_string(),
            "updated": Utc::now().to_rfc3339()
        });

        if let Err(e) = self
            .redis
            .setex(&cache_key, &volumes_data.to_string(), 300)
            .await
        {
            warn!(
                "Failed to cache volumes for consumer {}: {}",
                consumer_id, e
            );
        }

        Ok((daily, monthly))
    }

    async fn update_limits_cache(
        &self,
        consumer_id: Uuid,
        limits: &KycLimits,
    ) -> Result<(), KycLimitsError> {
        let cache_key = format!("kyc_limits:{}", consumer_id);
        let limits_data = serde_json::json!({
            "tier": format!("{:?}", limits.tier),
            "max_transaction_amount": limits.max_transaction_amount.to_string(),
            "daily_volume_limit": limits.daily_volume_limit.to_string(),
            "monthly_volume_limit": limits.monthly_volume_limit.to_string(),
            "daily_volume_used": limits.daily_volume_used.to_string(),
            "monthly_volume_used": limits.monthly_volume_used.to_string(),
            "updated": Utc::now().to_rfc3339()
        });

        if let Err(e) = self
            .redis
            .setex(&cache_key, &limits_data.to_string(), 300)
            .await
        {
            warn!("Failed to cache limits for consumer {}: {}", consumer_id, e);
        }

        Ok(())
    }

    async fn clear_limits_cache(&self, consumer_id: Uuid) -> Result<(), KycLimitsError> {
        let cache_key = format!("kyc_limits:{}", consumer_id);
        let volumes_key = format!("kyc_volumes:{}", consumer_id);

        let _: () = self
            .redis
            .del(&cache_key)
            .await
            .map_err(|e| KycLimitsError::RedisError(e.to_string()))?;
        let _: () = self
            .redis
            .del(&volumes_key)
            .await
            .map_err(|e| KycLimitsError::RedisError(e.to_string()))?;

        Ok(())
    }

    async fn clear_all_limits_cache(&self) -> Result<(), KycLimitsError> {
        // This is a simple implementation - in production you might want to use
        // Redis SCAN or pattern-based deletion for better performance
        let patterns = vec!["kyc_limits:*", "kyc_volumes:*"];

        for pattern in patterns {
            let keys: Vec<String> = self
                .redis
                .keys(pattern)
                .await
                .map_err(|e| KycLimitsError::RedisError(e.to_string()))?;

            if !keys.is_empty() {
                let _: () = self
                    .redis
                    .del(&keys)
                    .await
                    .map_err(|e| KycLimitsError::RedisError(e.to_string()))?;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct TransactionLimitCheckResult {
    pub is_allowed: bool,
    pub violations: Vec<LimitViolation>,
    pub current_tier: KycTier,
    pub effective_tier: KycTier,
    pub transaction_amount: BigDecimal,
    pub remaining_limits: crate::kyc::tier_requirements::RemainingLimits,
    pub updated_limits: Option<KycLimits>,
}

#[derive(Debug, Clone)]
pub struct KycLimitsInfo {
    pub tier: KycTier,
    pub effective_tier: KycTier,
    pub max_transaction_amount: BigDecimal,
    pub daily_volume_limit: BigDecimal,
    pub monthly_volume_limit: BigDecimal,
    pub daily_volume_used: BigDecimal,
    pub monthly_volume_used: BigDecimal,
    pub daily_remaining: BigDecimal,
    pub monthly_remaining: BigDecimal,
    pub last_daily_reset: NaiveDate,
    pub last_monthly_reset: NaiveDate,
    pub enhanced_due_diligence_active: bool,
}

#[derive(Debug, Clone)]
pub struct ConsumerLimitWarning {
    pub consumer_id: Uuid,
    pub tier: KycTier,
    pub effective_tier: KycTier,
    pub daily_usage_ratio: f64,
    pub monthly_usage_ratio: f64,
    pub daily_volume_used: BigDecimal,
    pub monthly_volume_used: BigDecimal,
    pub daily_volume_limit: BigDecimal,
    pub monthly_volume_limit: BigDecimal,
}

#[derive(Debug, thiserror::Error)]
pub enum KycLimitsError {
    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Redis error: {0}")]
    RedisError(String),

    #[error("KYC record not found")]
    KycRecordNotFound,

    #[error("Limits not found")]
    LimitsNotFound,

    #[error("Invalid amount")]
    InvalidAmount,
}

impl From<DatabaseError> for KycLimitsError {
    fn from(error: DatabaseError) -> Self {
        KycLimitsError::DatabaseError(error.to_string())
    }
}

impl From<redis::RedisError> for KycLimitsError {
    fn from(error: redis::RedisError) -> Self {
        KycLimitsError::RedisError(error.to_string())
    }
}

// Add missing import for BigDecimal::from_str
use std::str::FromStr;
