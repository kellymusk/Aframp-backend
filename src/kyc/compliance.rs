use bigdecimal::BigDecimal;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::database::kyc_repository::{
    EddStatus, EnhancedDueDiligenceCase, KycEventType, KycRecord, KycRepository, KycStatus, KycTier,
};
use crate::kyc::limits::KycLimitsEnforcer;
use crate::kyc::service::KycService;
use crate::metrics;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EddTriggerConfig {
    pub volume_spike_threshold: f64, // e.g., 5.0 = 5x normal volume
    pub high_risk_jurisdictions: Vec<String>, // Country codes
    pub structuring_threshold: i32,  // Number of small transactions
    pub structuring_timeframe_hours: i64, // Time window to detect structuring
    pub max_single_transaction: BigDecimal, // Trigger EDD for large transactions
    pub daily_volume_threshold: BigDecimal, // Trigger EDD for high daily volume
    pub rapid_succession_threshold: i32, // Many transactions in short time
    pub rapid_succession_minutes: i64, // Time window for rapid succession
}

impl Default for EddTriggerConfig {
    fn default() -> Self {
        Self {
            volume_spike_threshold: 5.0,
            high_risk_jurisdictions: vec![
                "AF".to_string(),
                "IR".to_string(),
                "KP".to_string(),
                "MM".to_string(),
                "SY".to_string(),
                "SS".to_string(),
            ],
            structuring_threshold: 10,
            structuring_timeframe_hours: 24,
            max_single_transaction: BigDecimal::from_str("50000.00").unwrap(),
            daily_volume_threshold: BigDecimal::from_str("100000.00").unwrap(),
            rapid_succession_threshold: 20,
            rapid_succession_minutes: 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EddTriggerResult {
    pub triggered: bool,
    pub trigger_reasons: Vec<String>,
    pub risk_factors: Vec<String>,
    pub recommended_actions: Vec<String>,
    pub severity: EddSeverity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EddSeverity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceReport {
    pub report_date: DateTime<Utc>,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub total_verifications: i64,
    pub verifications_by_tier: HashMap<KycTier, i64>,
    pub approval_rates: HashMap<KycTier, f64>,
    pub rejection_rates: HashMap<KycTier, f64>,
    pub average_processing_time_hours: f64,
    pub manual_review_cases: i64,
    pub edd_cases: i64,
    pub provider_performance: HashMap<String, ProviderPerformance>,
    pub high_risk_transactions: Vec<HighRiskTransaction>,
    pub compliance_alerts: Vec<ComplianceAlert>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderPerformance {
    pub name: String,
    pub total_submissions: i64,
    pub approved: i64,
    pub rejected: i64,
    pub manual_review: i64,
    pub average_processing_time_hours: f64,
    pub webhook_success_rate: f64,
    pub api_error_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighRiskTransaction {
    pub transaction_id: Uuid,
    pub consumer_id: Uuid,
    pub amount: BigDecimal,
    pub timestamp: DateTime<Utc>,
    pub risk_factors: Vec<String>,
    pub edd_triggered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceAlert {
    pub alert_id: Uuid,
    pub alert_type: ComplianceAlertType,
    pub severity: EddSeverity,
    pub consumer_id: Option<Uuid>,
    pub description: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub acknowledged: bool,
    pub acknowledged_by: Option<Uuid>,
    pub acknowledged_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComplianceAlertType {
    ManualReviewBacklog,
    ProviderWebhookFailure,
    HighVolumeSpike,
    SuspiciousPattern,
    RegulatoryThreshold,
    SystemAnomaly,
}

#[derive(Clone)]
pub struct ComplianceService {
    repository: KycRepository,
    kyc_service: KycService,
    limits_enforcer: KycLimitsEnforcer,
    config: EddTriggerConfig,
}

impl ComplianceService {
    pub fn new(
        repository: KycRepository,
        kyc_service: KycService,
        limits_enforcer: KycLimitsEnforcer,
        config: EddTriggerConfig,
    ) -> Self {
        Self {
            repository,
            kyc_service,
            limits_enforcer,
            config,
        }
    }

    /// Analyze transaction for EDD triggers
    pub async fn analyze_transaction_for_edd(
        &self,
        consumer_id: Uuid,
        transaction_amount: BigDecimal,
        transaction_metadata: Option<serde_json::Value>,
    ) -> Result<EddTriggerResult, ComplianceError> {
        info!(
            "Analyzing transaction for EDD triggers: consumer={}, amount={}",
            consumer_id, transaction_amount
        );

        let mut trigger_reasons = Vec::new();
        let mut risk_factors = Vec::new();
        let mut recommended_actions = Vec::new();
        let mut severity = EddSeverity::Low;

        // Get consumer's KYC record
        let kyc_record = self
            .repository
            .get_kyc_record_by_consumer(consumer_id)
            .await?
            .ok_or(ComplianceError::KycRecordNotFound)?;

        // Check 1: Large single transaction
        if transaction_amount > self.config.max_single_transaction {
            trigger_reasons.push(format!(
                "Large transaction: {} exceeds threshold {}",
                transaction_amount, self.config.max_single_transaction
            ));
            risk_factors("LARGE_TRANSACTION".to_string());
            recommended_actions.push("Enhanced due diligence review required".to_string());
            severity = EddSeverity::High;
        }

        // Check 2: Volume spike analysis
        if let Ok(volume_spike) = self
            .detect_volume_spike(consumer_id, &transaction_amount)
            .await
        {
            if volume_spike {
                trigger_reasons.push("Unusual volume spike detected".to_string());
                risk_factors.push("VOLUME_SPIKE".to_string());
                recommended_actions.push("Monitor transaction patterns closely".to_string());
                severity = std::cmp::max(severity, EddSeverity::Medium);
            }
        }

        // Check 3: High-risk jurisdiction
        if let Some(metadata) = &transaction_metadata {
            if let Some(country) = metadata.get("destination_country").and_then(|v| v.as_str()) {
                if self
                    .config
                    .high_risk_jurisdictions
                    .contains(&country.to_string())
                {
                    trigger_reasons.push(format!(
                        "Transaction to high-risk jurisdiction: {}",
                        country
                    ));
                    risk_factors.push("HIGH_RISK_JURISDICTION".to_string());
                    recommended_actions.push("Additional documentation required".to_string());
                    severity = std::cmp::max(severity, EddSeverity::Medium);
                }
            }
        }

        // Check 4: Transaction structuring
        if let Ok(structuring_detected) = self.detect_transaction_structuring(consumer_id).await {
            if structuring_detected {
                trigger_reasons.push("Potential transaction structuring detected".to_string());
                risk_factors.push("STRUCTURING".to_string());
                recommended_actions
                    .push("Detailed transaction history analysis required".to_string());
                severity = EddSeverity::High;
            }
        }

        // Check 5: Rapid succession
        if let Ok(rapid_succession) = self.detect_rapid_succession(consumer_id).await {
            if rapid_succession {
                trigger_reasons.push("Rapid succession of transactions detected".to_string());
                risk_factors.push("RAPID_SUCCESSION".to_string());
                recommended_actions.push("Immediate review recommended".to_string());
                severity = std::cmp::max(severity, EddSeverity::Medium);
            }
        }

        // Check 6: Daily volume threshold
        if let Ok((daily_volume, _)) = self.get_current_volumes(consumer_id).await {
            if daily_volume > self.config.daily_volume_threshold {
                trigger_reasons.push(format!(
                    "Daily volume {} exceeds threshold {}",
                    daily_volume, self.config.daily_volume_threshold
                ));
                risk_factors.push("HIGH_DAILY_VOLUME".to_string());
                recommended_actions.push("Enhanced monitoring required".to_string());
                severity = std::cmp::max(severity, EddSeverity::Medium);
            }
        }

        let triggered = !trigger_reasons.is_empty();

        // Record metrics
        if triggered {
            metrics::counter!("kyc_edd_triggers_total",
                "severity" => format!("{:?}", severity),
                "tier" => format!("{:?}", kyc_record.tier)
            )
            .increment(1);

            info!(
                "EDD triggered for consumer {}: {:?}",
                consumer_id, trigger_reasons
            );
        }

        Ok(EddTriggerResult {
            triggered,
            trigger_reasons,
            risk_factors,
            recommended_actions,
            severity,
        })
    }

    /// Trigger EDD case for a consumer
    pub async fn trigger_edd_case(
        &self,
        consumer_id: Uuid,
        trigger_reason: String,
        risk_factors: Vec<String>,
        severity: EddSeverity,
    ) -> Result<EnhancedDueDiligenceCase, ComplianceError> {
        info!(
            "Triggering EDD case for consumer {}: {}",
            consumer_id, trigger_reason
        );

        // Get KYC record
        let kyc_record = self
            .repository
            .get_kyc_record_by_consumer(consumer_id)
            .await?
            .ok_or(ComplianceError::KycRecordNotFound)?;

        // Create EDD case
        let edd_case_id = Uuid::new_v4();
        let now = Utc::now();

        sqlx::query!(
            r#"
            INSERT INTO enhanced_due_diligence_cases (
                id, consumer_id, kyc_record_id, trigger_reason, risk_factors,
                status, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
            edd_case_id,
            consumer_id,
            kyc_record.id,
            trigger_reason,
            &risk_factors,
            EddStatus::Active as EddStatus,
            now
        )
        .execute(&self.repository.pool)
        .await
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?;

        // Reduce consumer's effective tier based on severity
        let reduced_tier = match severity {
            EddSeverity::Critical | EddSeverity::High => KycTier::Basic,
            EddSeverity::Medium => {
                // Reduce by one tier if above Basic
                match kyc_record.effective_tier {
                    KycTier::Enhanced => KycTier::Standard,
                    KycTier::Standard => KycTier::Basic,
                    _ => KycTier::Basic,
                }
            }
            EddSeverity::Low => kyc_record.effective_tier, // No reduction for low severity
        };

        if reduced_tier != kyc_record.effective_tier {
            self.limits_enforcer
                .reduce_effective_tier(
                    consumer_id,
                    reduced_tier,
                    format!("EDD case triggered: {}", trigger_reason),
                )
                .await?;
        }

        // Log EDD trigger
        self.repository
            .create_event(
                consumer_id,
                Some(kyc_record.id),
                KycEventType::EnhancedDueDiligenceTriggered,
                Some(format!("EDD triggered: {}", trigger_reason)),
                None,
                Some(serde_json::json!({
                    "edd_case_id": edd_case_id,
                    "risk_factors": risk_factors,
                    "severity": format!("{:?}", severity),
                    "reduced_tier": format!("{:?}", reduced_tier)
                })),
            )
            .await?;

        // Create compliance alert
        self.create_compliance_alert(
            ComplianceAlertType::SuspiciousPattern,
            severity,
            Some(consumer_id),
            format!("EDD case triggered: {}", trigger_reason),
            serde_json::json!({
                "edd_case_id": edd_case_id,
                "risk_factors": risk_factors,
                "severity": format!("{:?}", severity)
            }),
        )
        .await?;

        // Notify compliance team
        // TODO: Implement notification system

        // Retrieve and return the created case
        let edd_case = sqlx::query_as::<_, EnhancedDueDiligenceCase>(
            "SELECT * FROM enhanced_due_diligence_cases WHERE id = $1"
        )
        .bind(edd_case_id)
        .fetch_one(&self.repository.pool)
        .await
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?;

        metrics::counter!("kyc_edd_cases_created_total",
            "severity" => format!("{:?}", severity),
            "tier" => format!("{:?}", kyc_record.tier)
        )
        .increment(1);

        Ok(edd_case)
    }

    /// Generate compliance report
    pub async fn generate_compliance_report(
        &self,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
    ) -> Result<ComplianceReport, ComplianceError> {
        info!(
            "Generating compliance report for period {} to {}",
            period_start, period_end
        );

        // Get verification statistics
        let total_verifications = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) FROM kyc_records 
            WHERE created_at BETWEEN $1 AND $2
            "#,
            period_start,
            period_end
        )
        .fetch_one(&self.repository.pool)
        .await
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?
        .unwrap_or(0);

        let verifications_by_tier = sqlx::query(
            r#"
            SELECT tier, COUNT(*) as count
            FROM kyc_records 
            WHERE created_at BETWEEN $1 AND $2
            GROUP BY tier
            "#
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_all(&self.repository.pool)
        .await
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?;

        let mut tier_counts = HashMap::new();
        for record in verifications_by_tier {
            let tier: KycTier = record.try_get("tier")
                .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?;
            let count: i64 = record.try_get::<i64, _>("count")
                .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?;
            tier_counts.insert(tier, count);
        }

        // Calculate approval/rejection rates
        let mut approval_rates = HashMap::new();
        let mut rejection_rates = HashMap::new();

        for (tier, _) in &tier_counts {
            let stats = sqlx::query!(
                r#"
                SELECT 
                    COUNT(*) FILTER (WHERE status = 'approved') as approved,
                    COUNT(*) FILTER (WHERE status = 'rejected') as rejected
                FROM kyc_records 
                WHERE tier = $1 AND created_at BETWEEN $2 AND $3
                "#,
                tier as KycTier,
                period_start,
                period_end
            )
            .fetch_one(&self.repository.pool)
            .await
            .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?;

            let total = stats.approved.unwrap_or(0) + stats.rejected.unwrap_or(0);
            if total > 0 {
                approval_rates.insert(*tier, stats.approved.unwrap_or(0) as f64 / total as f64);
                rejection_rates.insert(*tier, stats.rejected.unwrap_or(0) as f64 / total as f64);
            }
        }

        // Get manual review cases
        let manual_review_cases = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) FROM manual_review_queue 
            WHERE created_at BETWEEN $1 AND $2
            "#,
            period_start,
            period_end
        )
        .fetch_one(&self.repository.pool)
        .await
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?
        .unwrap_or(0);

        // Get EDD cases
        let edd_cases = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) FROM enhanced_due_diligence_cases 
            WHERE created_at BETWEEN $1 AND $2
            "#,
            period_start,
            period_end
        )
        .fetch_one(&self.repository.pool)
        .await
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?
        .unwrap_or(0);

        // Get provider performance
        let provider_performance = self
            .calculate_provider_performance(period_start, period_end)
            .await?;

        // Get high-risk transactions
        let high_risk_transactions = self
            .get_high_risk_transactions(period_start, period_end)
            .await?;

        // Get compliance alerts
        let compliance_alerts = self.get_compliance_alerts(period_start, period_end).await?;

        Ok(ComplianceReport {
            report_date: Utc::now(),
            period_start,
            period_end,
            total_verifications,
            verifications_by_tier: tier_counts,
            approval_rates,
            rejection_rates,
            average_processing_time_hours: 0.0, // TODO: Calculate actual processing time
            manual_review_cases,
            edd_cases,
            provider_performance,
            high_risk_transactions,
            compliance_alerts,
        })
    }

    /// Export KYC audit trail for regulatory inspection
    pub async fn export_audit_trail(
        &self,
        consumer_id: Uuid,
        format: AuditExportFormat,
    ) -> Result<String, ComplianceError> {
        info!(
            "Exporting audit trail for consumer {} in format {:?}",
            consumer_id, format
        );

        // Get complete KYC history
        let kyc_records = sqlx::query_as::<_, KycRecord>(
            "SELECT * FROM kyc_records WHERE consumer_id = $1 ORDER BY created_at DESC"
        )
        .bind(consumer_id)
        .fetch_all(&self.repository.pool)
        .await
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?;

        let events = self
            .repository
            .get_events_by_consumer(consumer_id, None)
            .await?;
        let documents = if let Some(latest_record) = kyc_records.first() {
            self.repository
                .get_documents_by_kyc_record(latest_record.id)
                .await?
        } else {
            vec![]
        };

        let audit_trail = serde_json::json!({
            "consumer_id": consumer_id,
            "export_timestamp": Utc::now().to_rfc3339(),
            "format": format!("{:?}", format),
            "kyc_records": kyc_records,
            "events": events,
            "documents": documents,
            "export_version": "1.0"
        });

        match format {
            AuditExportFormat::Json => Ok(serde_json::to_string_pretty(&audit_trail)
                .map_err(|e| ComplianceError::SerializationError(e.to_string()))?),
            AuditExportFormat::Csv => {
                // Convert to CSV format (simplified)
                self.convert_to_csv(&audit_trail).await
            }
        }
    }

    // Private helper methods
    async fn detect_volume_spike(
        &self,
        consumer_id: Uuid,
        current_amount: &BigDecimal,
    ) -> Result<bool, ComplianceError> {
        // Get average daily volume for the past 30 days
        let avg_volume = sqlx::query_scalar!(
            r#"
            SELECT AVG(daily_volume) as avg_volume
            FROM kyc_volume_trackers
            WHERE consumer_id = $1 AND date >= CURRENT_DATE - INTERVAL '30 days'
            "#,
            consumer_id
        )
        .fetch_one(&self.repository.pool)
        .await
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?
        .flatten();

        if let Some(avg) = avg_volume {
            let threshold = avg
                * BigDecimal::from_str(&self.config.volume_spike_threshold.to_string()).unwrap();
            Ok(current_amount > &threshold)
        } else {
            Ok(false) // No historical data
        }
    }

    async fn detect_transaction_structuring(
        &self,
        consumer_id: Uuid,
    ) -> Result<bool, ComplianceError> {
        let threshold = BigDecimal::from_str("1000.00").unwrap(); // Small transactions threshold
        let time_window = Utc::now() - Duration::hours(self.config.structuring_timeframe_hours);

        let count = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*)
            FROM transactions t
            WHERE t.consumer_id = $1 
            AND t.from_amount < $2
            AND t.created_at > $3
            "#,
            consumer_id,
            threshold,
            time_window
        )
        .fetch_one(&self.repository.pool)
        .await
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?
        .unwrap_or(0);

        Ok(count >= self.config.structuring_threshold)
    }

    async fn detect_rapid_succession(&self, consumer_id: Uuid) -> Result<bool, ComplianceError> {
        let time_window = Utc::now() - Duration::minutes(self.config.rapid_succession_minutes);

        let count = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*)
            FROM transactions t
            WHERE t.consumer_id = $1 
            AND t.created_at > $2
            "#,
            consumer_id,
            time_window
        )
        .fetch_one(&self.repository.pool)
        .await
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?
        .unwrap_or(0);

        Ok(count >= self.config.rapid_succession_threshold)
    }

    async fn get_current_volumes(
        &self,
        consumer_id: Uuid,
    ) -> Result<(BigDecimal, BigDecimal), ComplianceError> {
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
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?;

        match result {
            Some(record) => Ok((record.daily_volume, record.monthly_volume)),
            None => Ok((
                BigDecimal::from_str("0").unwrap(),
                BigDecimal::from_str("0").unwrap(),
            )),
        }
    }

    async fn calculate_provider_performance(
        &self,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
    ) -> Result<HashMap<String, ProviderPerformance>, ComplianceError> {
        // This is a simplified implementation
        // In practice, you'd aggregate data from various sources
        let providers = sqlx::query!(
            r#"
            SELECT DISTINCT verification_provider
            FROM kyc_records
            WHERE verification_provider IS NOT NULL
            AND created_at BETWEEN $1 AND $2
            "#,
            period_start,
            period_end
        )
        .fetch_all(&self.repository.pool)
        .await
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?;

        let mut performance = HashMap::new();

        for provider_record in providers {
            if let Some(provider_name) = provider_record.verification_provider {
                let stats = sqlx::query!(
                    r#"
                    SELECT 
                        COUNT(*) as total,
                        COUNT(*) FILTER (WHERE status = 'approved') as approved,
                        COUNT(*) FILTER (WHERE status = 'rejected') as rejected,
                        COUNT(*) FILTER (WHERE status = 'manual_review') as manual_review
                    FROM kyc_records
                    WHERE verification_provider = $1
                    AND created_at BETWEEN $2 AND $3
                    "#,
                    provider_name,
                    period_start,
                    period_end
                )
                .fetch_one(&self.repository.pool)
                .await
                .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?;

                performance.insert(
                    provider_name.clone(),
                    ProviderPerformance {
                        name: provider_name,
                        total_submissions: stats.total.unwrap_or(0),
                        approved: stats.approved.unwrap_or(0),
                        rejected: stats.rejected.unwrap_or(0),
                        manual_review: stats.manual_review.unwrap_or(0),
                        average_processing_time_hours: 0.0, // TODO: Calculate actual processing time
                        webhook_success_rate: 0.95,         // TODO: Calculate actual rate
                        api_error_rate: 0.02,               // TODO: Calculate actual rate
                    },
                );
            }
        }

        Ok(performance)
    }

    async fn get_high_risk_transactions(
        &self,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
    ) -> Result<Vec<HighRiskTransaction>, ComplianceError> {
        // Simplified implementation - would join with transaction analysis
        Ok(vec![])
    }

    async fn get_compliance_alerts(
        &self,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
    ) -> Result<Vec<ComplianceAlert>, ComplianceError> {
        let alerts = sqlx::query_as!(
            ComplianceAlert,
            r#"
            SELECT * FROM compliance_alerts
            WHERE created_at BETWEEN $1 AND $2
            ORDER BY created_at DESC
            "#,
            period_start,
            period_end
        )
        .fetch_all(&self.repository.pool)
        .await
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?;

        Ok(alerts)
    }

    async fn create_compliance_alert(
        &self,
        alert_type: ComplianceAlertType,
        severity: EddSeverity,
        consumer_id: Option<Uuid>,
        description: String,
        metadata: serde_json::Value,
    ) -> Result<(), ComplianceError> {
        let alert_id = Uuid::new_v4();
        let now = Utc::now();

        sqlx::query!(
            r#"
            INSERT INTO compliance_alerts (
                id, alert_type, severity, consumer_id, description,
                metadata, created_at, acknowledged
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
            alert_id,
            alert_type as ComplianceAlertType,
            severity as EddSeverity,
            consumer_id,
            description,
            metadata,
            now,
            false
        )
        .execute(&self.repository.pool)
        .await
        .map_err(|e| ComplianceError::DatabaseError(e.to_string()))?;

        metrics::counter!("compliance_alerts_total",
            "type" => format!("{:?}", alert_type),
            "severity" => format!("{:?}", severity)
        )
        .increment(1);

        Ok(())
    }

    async fn convert_to_csv(
        &self,
        audit_trail: &serde_json::Value,
    ) -> Result<String, ComplianceError> {
        // Simplified CSV conversion
        let mut csv = String::new();
        csv.push_str("timestamp,event_type,consumer_id,details\n");

        if let Some(events) = audit_trail.get("events").and_then(|e| e.as_array()) {
            for event in events {
                if let (Some(timestamp), Some(event_type), Some(consumer_id)) = (
                    event.get("timestamp").and_then(|t| t.as_str()),
                    event.get("event_type").and_then(|t| t.as_str()),
                    event.get("consumer_id").and_then(|c| c.as_str()),
                ) {
                    csv.push_str(&format!(
                        "{},{},{},\"{}\"\n",
                        timestamp,
                        event_type,
                        consumer_id,
                        event
                            .get("event_detail")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                    ));
                }
            }
        }

        Ok(csv)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditExportFormat {
    Json,
    Csv,
}

#[derive(Debug, thiserror::Error)]
pub enum ComplianceError {
    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("KYC record not found")]
    KycRecordNotFound,

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Configuration error: {0}")]
    ConfigurationError(String),
}
