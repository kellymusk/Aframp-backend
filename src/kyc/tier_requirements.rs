use crate::database::kyc_repository::{DocumentType, KycTier, KycTierDefinition};
use bigdecimal::BigDecimal;
use std::collections::HashMap;
use std::str::FromStr;

pub struct KycTierRequirements;

impl KycTierRequirements {
    pub fn get_tier_definition(tier: KycTier) -> KycTierDefinition {
        match tier {
            KycTier::Unverified => KycTierDefinition {
                tier: KycTier::Unverified,
                name: "Tier 0 - Unverified".to_string(),
                description: "Sandbox and read-only access only".to_string(),
                required_documents: vec![],
                max_transaction_amount: BigDecimal::from_str("0").unwrap(),
                daily_volume_limit: BigDecimal::from_str("0").unwrap(),
                monthly_volume_limit: BigDecimal::from_str("0").unwrap(),
                requires_enhanced_due_diligence: false,
                cooling_off_period_days: 0,
            },
            KycTier::Basic => KycTierDefinition {
                tier: KycTier::Basic,
                name: "Tier 1 - Basic".to_string(),
                description: "Basic identity verification with limited transaction volumes".to_string(),
                required_documents: vec![
                    DocumentType::NationalId,
                    DocumentType::Passport,
                    DocumentType::DriversLicense,
                ],
                max_transaction_amount: BigDecimal::from_str("1000.00").unwrap(),
                daily_volume_limit: BigDecimal::from_str("5000.00").unwrap(),
                monthly_volume_limit: BigDecimal::from_str("50000.00").unwrap(),
                requires_enhanced_due_diligence: false,
                cooling_off_period_days: 7,
            },
            KycTier::Standard => KycTierDefinition {
                tier: KycTier::Standard,
                name: "Tier 2 - Standard".to_string(),
                description: "Full identity and address verification with standard transaction volumes".to_string(),
                required_documents: vec![
                    DocumentType::NationalId,
                    DocumentType::Passport,
                    DocumentType::DriversLicense,
                    DocumentType::UtilityBill,
                    DocumentType::BankStatement,
                    DocumentType::GovernmentLetter,
                ],
                max_transaction_amount: BigDecimal::from_str("10000.00").unwrap(),
                daily_volume_limit: BigDecimal::from_str("50000.00").unwrap(),
                monthly_volume_limit: BigDecimal::from_str("500000.00").unwrap(),
                requires_enhanced_due_diligence: false,
                cooling_off_period_days: 14,
            },
            KycTier::Enhanced => KycTierDefinition {
                tier: KycTier::Enhanced,
                name: "Tier 3 - Enhanced".to_string(),
                description: "Enhanced due diligence with elevated transaction volumes for high-value consumers".to_string(),
                required_documents: vec![
                    DocumentType::NationalId,
                    DocumentType::Passport,
                    DocumentType::DriversLicense,
                    DocumentType::UtilityBill,
                    DocumentType::BankStatement,
                    DocumentType::GovernmentLetter,
                    DocumentType::SourceOfFunds,
                    DocumentType::BusinessRegistration,
                ],
                max_transaction_amount: BigDecimal::from_str("100000.00").unwrap(),
                daily_volume_limit: BigDecimal::from_str("500000.00").unwrap(),
                monthly_volume_limit: BigDecimal::from_str("5000000.00").unwrap(),
                requires_enhanced_due_diligence: true,
                cooling_off_period_days: 30,
            },
        }
    }

    pub fn validate_tier_requirements(
        tier: KycTier,
        submitted_documents: &[DocumentType],
    ) -> TierValidationResult {
        let definition = Self::get_tier_definition(tier);
        let required_docs = definition.required_documents;

        let missing_docs: Vec<DocumentType> = required_docs
            .iter()
            .filter(|&req_doc| !submitted_documents.contains(req_doc))
            .cloned()
            .collect();

        let extra_docs: Vec<DocumentType> = submitted_documents
            .iter()
            .filter(|&sub_doc| !required_docs.contains(sub_doc))
            .cloned()
            .collect();

        TierValidationResult {
            is_valid: missing_docs.is_empty(),
            missing_documents: missing_docs,
            extra_documents: extra_docs,
            tier,
            required_documents: required_docs,
        }
    }

    pub fn get_next_tier(current_tier: KycTier) -> Option<KycTier> {
        match current_tier {
            KycTier::Unverified => Some(KycTier::Basic),
            KycTier::Basic => Some(KycTier::Standard),
            KycTier::Standard => Some(KycTier::Enhanced),
            KycTier::Enhanced => None, // Already at highest tier
        }
    }

    pub fn can_upgrade_to_tier(
        current_tier: KycTier,
        target_tier: KycTier,
        submitted_documents: &[DocumentType],
    ) -> bool {
        if target_tier == current_tier {
            return true;
        }

        // Check if target tier is higher than current
        let tier_order = vec![
            KycTier::Unverified,
            KycTier::Basic,
            KycTier::Standard,
            KycTier::Enhanced,
        ];

        let current_index = tier_order
            .iter()
            .position(|&t| t == current_tier)
            .unwrap_or(0);
        let target_index = tier_order
            .iter()
            .position(|&t| t == target_tier)
            .unwrap_or(0);

        if target_index <= current_index {
            return false;
        }

        // Validate requirements for target tier
        let validation = Self::validate_tier_requirements(target_tier, submitted_documents);
        validation.is_valid
    }

    pub fn get_minimum_documents_for_tier(tier: KycTier) -> Vec<DocumentType> {
        Self::get_tier_definition(tier).required_documents
    }

    pub fn is_document_required_for_tier(document_type: DocumentType, tier: KycTier) -> bool {
        let required_docs = Self::get_minimum_documents_for_tier(tier);
        required_docs.contains(&document_type)
    }

    pub fn get_tier_limits(tier: KycTier) -> TierLimits {
        let definition = Self::get_tier_definition(tier);
        TierLimits {
            max_transaction_amount: definition.max_transaction_amount,
            daily_volume_limit: definition.daily_volume_limit,
            monthly_volume_limit: definition.monthly_volume_limit,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TierValidationResult {
    pub is_valid: bool,
    pub missing_documents: Vec<DocumentType>,
    pub extra_documents: Vec<DocumentType>,
    pub tier: KycTier,
    pub required_documents: Vec<DocumentType>,
}

#[derive(Debug, Clone)]
pub struct TierLimits {
    pub max_transaction_amount: BigDecimal,
    pub daily_volume_limit: BigDecimal,
    pub monthly_volume_limit: BigDecimal,
}

#[derive(Debug, Clone)]
pub struct TransactionLimitEnforcer {
    daily_limit: BigDecimal,
    monthly_limit: BigDecimal,
    max_single_transaction: BigDecimal,
}

impl TransactionLimitEnforcer {
    pub fn new(tier: KycTier) -> Self {
        let limits = KycTierRequirements::get_tier_limits(tier);
        Self {
            daily_limit: limits.daily_volume_limit,
            monthly_limit: limits.monthly_volume_limit,
            max_single_transaction: limits.max_transaction_amount,
        }
    }

    pub fn check_transaction_limits(
        &self,
        transaction_amount: BigDecimal,
        daily_volume_used: BigDecimal,
        monthly_volume_used: BigDecimal,
    ) -> TransactionLimitResult {
        let mut violations = vec![];

        // Check single transaction limit
        if transaction_amount > self.max_single_transaction {
            violations.push(LimitViolation::SingleTransactionLimit {
                amount: transaction_amount.clone(),
                limit: self.max_single_transaction.clone(),
            });
        }

        // Check daily volume limit
        let new_daily_volume = &daily_volume_used + &transaction_amount;
        if new_daily_volume > self.daily_limit {
            violations.push(LimitViolation::DailyVolumeLimit {
                attempted_volume: new_daily_volume,
                limit: self.daily_limit.clone(),
                current_used: daily_volume_used,
            });
        }

        // Check monthly volume limit
        let new_monthly_volume = &monthly_volume_used + &transaction_amount;
        if new_monthly_volume > self.monthly_limit {
            violations.push(LimitViolation::MonthlyVolumeLimit {
                attempted_volume: new_monthly_volume,
                limit: self.monthly_limit.clone(),
                current_used: monthly_volume_used,
            });
        }

        TransactionLimitResult {
            is_allowed: violations.is_empty(),
            violations,
            transaction_amount,
            daily_remaining: &self.daily_limit - &daily_volume_used,
            monthly_remaining: &self.monthly_limit - &monthly_volume_used,
        }
    }

    pub fn get_remaining_limits(
        &self,
        daily_volume_used: BigDecimal,
        monthly_volume_used: BigDecimal,
    ) -> RemainingLimits {
        RemainingLimits {
            single_transaction: self.max_single_transaction.clone(),
            daily_volume: &self.daily_limit - &daily_volume_used,
            monthly_volume: &self.monthly_limit - &monthly_volume_used,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TransactionLimitResult {
    pub is_allowed: bool,
    pub violations: Vec<LimitViolation>,
    pub transaction_amount: BigDecimal,
    pub daily_remaining: BigDecimal,
    pub monthly_remaining: BigDecimal,
}

#[derive(Debug, Clone)]
pub enum LimitViolation {
    SingleTransactionLimit {
        amount: BigDecimal,
        limit: BigDecimal,
    },
    DailyVolumeLimit {
        attempted_volume: BigDecimal,
        limit: BigDecimal,
        current_used: BigDecimal,
    },
    MonthlyVolumeLimit {
        attempted_volume: BigDecimal,
        limit: BigDecimal,
        current_used: BigDecimal,
    },
}

#[derive(Debug, Clone)]
pub struct RemainingLimits {
    pub single_transaction: BigDecimal,
    pub daily_volume: BigDecimal,
    pub monthly_volume: BigDecimal,
}

pub struct VolumeTracker {
    consumer_id: uuid::Uuid,
}

impl VolumeTracker {
    pub fn new(consumer_id: uuid::Uuid) -> Self {
        Self { consumer_id }
    }

    pub async fn reset_daily_counters(&self, pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
        let today = chrono::Utc::now().date_naive();

        sqlx::query!(
            r#"
            INSERT INTO kyc_volume_trackers (consumer_id, date, daily_volume, monthly_volume, transaction_count, last_updated)
            VALUES ($1, $2, 0, 0, 0, NOW())
            ON CONFLICT (consumer_id, date) DO UPDATE SET
                daily_volume = 0,
                transaction_count = 0,
                last_updated = NOW()
            "#,
            self.consumer_id,
            today
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn reset_monthly_counters(&self, pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
        let today = chrono::Utc::now().date_naive();

        sqlx::query!(
            r#"
            INSERT INTO kyc_volume_trackers (consumer_id, date, daily_volume, monthly_volume, transaction_count, last_updated)
            VALUES ($1, $2, 0, 0, 0, NOW())
            ON CONFLICT (consumer_id, date) DO UPDATE SET
                monthly_volume = 0,
                last_updated = NOW()
            "#,
            self.consumer_id,
            today
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn get_current_volumes(
        &self,
        pool: &sqlx::PgPool,
    ) -> Result<(BigDecimal, BigDecimal), sqlx::Error> {
        let today = chrono::Utc::now().date_naive();

        let result = sqlx::query!(
            r#"
            SELECT COALESCE(daily_volume, '0'::numeric) as daily_volume,
                   COALESCE(monthly_volume, '0'::numeric) as monthly_volume
            FROM kyc_volume_trackers
            WHERE consumer_id = $1 AND date = $2
            "#,
            self.consumer_id,
            today
        )
        .fetch_optional(pool)
        .await?;

        match result {
            Some(record) => Ok((record.daily_volume, record.monthly_volume)),
            None => Ok((
                BigDecimal::from_str("0").unwrap(),
                BigDecimal::from_str("0").unwrap(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigdecimal::BigDecimal;
    use std::str::FromStr;

    #[test]
    fn test_tier_validation_basic() {
        let submitted_docs = vec![DocumentType::NationalId];
        let result =
            KycTierRequirements::validate_tier_requirements(KycTier::Basic, &submitted_docs);

        assert!(result.is_valid);
        assert!(result.missing_documents.is_empty());
    }

    #[test]
    fn test_tier_validation_missing_docs() {
        let submitted_docs = vec![DocumentType::NationalId];
        let result =
            KycTierRequirements::validate_tier_requirements(KycTier::Standard, &submitted_docs);

        assert!(!result.is_valid);
        assert!(!result.missing_documents.is_empty());
    }

    #[test]
    fn test_transaction_limit_enforcer() {
        let enforcer = TransactionLimitEnforcer::new(KycTier::Basic);
        let amount = BigDecimal::from_str("500.00").unwrap();
        let daily_used = BigDecimal::from_str("1000.00").unwrap();
        let monthly_used = BigDecimal::from_str("10000.00").unwrap();

        let result = enforcer.check_transaction_limits(amount, daily_used, monthly_used);
        assert!(result.is_allowed);
    }

    #[test]
    fn test_transaction_limit_violation() {
        let enforcer = TransactionLimitEnforcer::new(KycTier::Basic);
        let amount = BigDecimal::from_str("2000.00").unwrap(); // Exceeds single transaction limit
        let daily_used = BigDecimal::from_str("0.00").unwrap();
        let monthly_used = BigDecimal::from_str("0.00").unwrap();

        let result = enforcer.check_transaction_limits(amount, daily_used, monthly_used);
        assert!(!result.is_allowed);
        assert!(!result.violations.is_empty());
    }
}
