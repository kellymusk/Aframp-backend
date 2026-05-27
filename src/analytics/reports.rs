use super::models::*;
use super::repository::AnalyticsRepository;
use chrono::{DateTime, Datelike, Duration, Utc};
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

pub struct ReportGenerator {
    pool: Arc<PgPool>,
    repo: Arc<AnalyticsRepository>,
}

impl ReportGenerator {
    pub fn new(pool: Arc<PgPool>, repo: Arc<AnalyticsRepository>) -> Self {
        Self { pool, repo }
    }

    /// Generate weekly platform usage report
    pub async fn generate_weekly_platform_report(
        &self,
    ) -> Result<PlatformUsageReport, anyhow::Error> {
        let now = Utc::now();
        let period_end = now;
        let period_start = now - Duration::days(7);

        self.generate_platform_report("weekly", period_start, period_end)
            .await
    }

    /// Generate monthly platform usage report
    pub async fn generate_monthly_platform_report(
        &self,
    ) -> Result<PlatformUsageReport, anyhow::Error> {
        let now = Utc::now();
        let period_end = now;
        let period_start = now - Duration::days(30);

        self.generate_platform_report("monthly", period_start, period_end)
            .await
    }

    async fn generate_platform_report(
        &self,
        report_type: &str,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
    ) -> Result<PlatformUsageReport, anyhow::Error> {
        info!(
            report_type,
            ?period_start,
            ?period_end,
            "Generating platform usage report"
        );

        // Total API requests
        let total_api_requests = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) as "count!"
            FROM api_audit_logs
            WHERE created_at >= $1 AND created_at < $2
            "#,
            period_start,
            period_end
        )
        .fetch_one(&*self.pool)
        .await?;

        // Platform error rate
        let error_stats = sqlx::query!(
            r#"
            SELECT 
                COUNT(*) as "total!",
                COUNT(*) FILTER (WHERE outcome = 'failure') as "failures!"
            FROM api_audit_logs
            WHERE created_at >= $1 AND created_at < $2
            "#,
            period_start,
            period_end
        )
        .fetch_one(&*self.pool)
        .await?;

        let platform_error_rate = if error_stats.total > 0 {
            error_stats.failures as f64 / error_stats.total as f64
        } else {
            0.0
        };

        // Consumer counts
        let total_consumers = sqlx::query_scalar!(
            r#"
            SELECT COUNT(DISTINCT actor_id) as "count!"
            FROM api_audit_logs
            WHERE actor_type = 'consumer' AND actor_id IS NOT NULL
            "#
        )
        .fetch_one(&*self.pool)
        .await? as i32;

        let active_consumers = sqlx::query_scalar!(
            r#"
            SELECT COUNT(DISTINCT actor_id) as "count!"
            FROM api_audit_logs
            WHERE created_at >= $1 
              AND created_at < $2
              AND actor_type = 'consumer' 
              AND actor_id IS NOT NULL
            "#,
            period_start,
            period_end
        )
        .fetch_one(&*self.pool)
        .await? as i32;

        // New consumers (first seen in this period)
        let new_consumers = sqlx::query_scalar!(
            r#"
            SELECT COUNT(DISTINCT actor_id) as "count!"
            FROM api_audit_logs
            WHERE actor_type = 'consumer' 
              AND actor_id IS NOT NULL
              AND actor_id NOT IN (
                  SELECT DISTINCT actor_id 
                  FROM api_audit_logs 
                  WHERE created_at < $1 
                    AND actor_type = 'consumer' 
                    AND actor_id IS NOT NULL
              )
              AND created_at >= $1 
              AND created_at < $2
            "#,
            period_start,
            period_end
        )
        .fetch_one(&*self.pool)
        .await? as i32;

        // At-risk consumers
        let at_risk_consumers = sqlx::query_scalar!(
            r#"
            SELECT COUNT(DISTINCT consumer_id) as "count!"
            FROM consumer_health_scores
            WHERE is_at_risk = true
              AND score_timestamp >= $1
            "#,
            period_start
        )
        .fetch_one(&*self.pool)
        .await? as i32;

        // Feature adoption summary
        let feature_adoption = sqlx::query!(
            r#"
            SELECT 
                feature_name,
                COUNT(DISTINCT consumer_id) as "consumer_count!"
            FROM consumer_feature_adoption
            WHERE is_active = true
            GROUP BY feature_name
            "#
        )
        .fetch_all(&*self.pool)
        .await?;

        let feature_adoption_summary = json!(feature_adoption
            .into_iter()
            .map(|r| json!({
                "feature": r.feature_name,
                "active_consumers": r.consumer_count
            }))
            .collect::<Vec<_>>());

        // Top consumers by volume
        let top_consumers = sqlx::query!(
            r#"
            SELECT 
                actor_id,
                COUNT(*) as "request_count!"
            FROM api_audit_logs
            WHERE created_at >= $1 
              AND created_at < $2
              AND actor_type = 'consumer'
              AND actor_id IS NOT NULL
            GROUP BY actor_id
            ORDER BY COUNT(*) DESC
            LIMIT 10
            "#,
            period_start,
            period_end
        )
        .fetch_all(&*self.pool)
        .await?;

        let top_consumers_by_volume = json!(top_consumers
            .into_iter()
            .enumerate()
            .map(|(idx, r)| json!({
                "rank": idx + 1,
                "consumer_id": r.actor_id,
                "request_count": r.request_count
            }))
            .collect::<Vec<_>>());

        let report = PlatformUsageReport {
            id: Uuid::new_v4(),
            report_type: report_type.to_string(),
            report_period_start: period_start,
            report_period_end: period_end,
            total_api_requests,
            platform_error_rate,
            total_consumers,
            active_consumers,
            new_consumers,
            at_risk_consumers,
            feature_adoption_summary: Some(feature_adoption_summary),
            top_consumers_by_volume: Some(top_consumers_by_volume),
            report_file_path: None,
            report_file_size_bytes: None,
            generated_at: Utc::now(),
            created_at: Utc::now(),
        };

        self.repo.insert_platform_report(&report).await?;

        info!(
            report_type,
            total_api_requests, active_consumers, "Platform usage report generated"
        );

        Ok(report)
    }

    /// Generate monthly consumer report
    pub async fn generate_consumer_monthly_report(
        &self,
        consumer_id: &str,
    ) -> Result<ConsumerMonthlyReport, anyhow::Error> {
        let now = Utc::now();
        let report_month = chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
            .ok_or_else(|| anyhow::anyhow!("Invalid date"))?;

        let period_start = report_month
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow::anyhow!("Invalid datetime"))?
            .and_local_timezone(Utc)
            .unwrap();

        let period_end = now;

        // Get usage snapshot
        let snapshots = self
            .repo
            .get_consumer_snapshots(consumer_id, SnapshotPeriod::Monthly, 1)
            .await?;

        let (total_requests, error_rate, avg_response_time_ms) =
            if let Some(snapshot) = snapshots.first() {
                (
                    snapshot.total_requests,
                    snapshot.error_rate,
                    snapshot.avg_response_time_ms,
                )
            } else {
                (0, 0.0, 0)
            };

        // Get health score
        let health_score = self
            .repo
            .get_latest_health_score(consumer_id)
            .await?
            .map(|s| s.health_score)
            .unwrap_or(100);

        // Get features used
        let features = self.repo.get_consumer_feature_adoption(consumer_id).await?;

        let features_used = json!(features
            .into_iter()
            .map(|f| json!({
                "feature": f.feature_name,
                "usage_count": f.total_usage_count,
                "last_used": f.last_used_at
            }))
            .collect::<Vec<_>>());

        let integration_health_summary = if health_score >= 80 {
            Some("Excellent - Your integration is performing well".to_string())
        } else if health_score >= 60 {
            Some("Good - Minor issues detected, review recommended".to_string())
        } else {
            Some("Needs Attention - Please review error rates and activity".to_string())
        };

        let report = ConsumerMonthlyReport {
            id: Uuid::new_v4(),
            consumer_id: consumer_id.to_string(),
            report_month,
            total_requests,
            error_rate,
            avg_response_time_ms,
            health_score,
            features_used: Some(features_used),
            integration_health_summary,
            report_file_path: None,
            report_file_size_bytes: None,
            delivered_at: None,
            generated_at: Utc::now(),
            created_at: Utc::now(),
        };

        Ok(report)
    }
}
