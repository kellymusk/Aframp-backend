//! Compliance Effectiveness Repository — Data Aggregation & Persistence

use super::models::{ComplianceMetrics, ComplianceReport, ListReportsQuery, ReportListPage, ReportSchedule, TrendDirection};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

pub struct ComplianceEffectivenessRepository {
    pool: PgPool,
}

impl ComplianceEffectivenessRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // ── KPI Aggregation ───────────────────────────────────────────────────────

    /// Aggregate compliance KPIs for the given period from aml_cases table.
    pub async fn aggregate_metrics(
        &self,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
    ) -> Result<ComplianceMetrics, anyhow::Error> {
        // Total alerts
        let total_alerts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aml_cases WHERE created_at >= $1 AND created_at < $2"
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_one(&self.pool)
        .await?;

        // Alert breakdown by flag type (parse flags_json)
        let sanctions_alerts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aml_cases 
             WHERE created_at >= $1 AND created_at < $2 
             AND flags_json::text LIKE '%SanctionsHit%'"
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_one(&self.pool)
        .await?;

        let aml_alerts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aml_cases 
             WHERE created_at >= $1 AND created_at < $2 
             AND (flags_json::text LIKE '%SmurfingDetected%' OR flags_json::text LIKE '%RapidFlip%')"
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_one(&self.pool)
        .await?;

        let kyc_alerts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aml_cases 
             WHERE created_at >= $1 AND created_at < $2 
             AND flags_json::text LIKE '%HighCorridorRisk%'"
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_one(&self.pool)
        .await?;

        // False positives (cleared cases with LOW flag level)
        let false_positives: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aml_cases 
             WHERE created_at >= $1 AND created_at < $2 
             AND status = 'Cleared' AND flag_level = 'LOW'"
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_one(&self.pool)
        .await?;

        let false_positive_rate = if total_alerts > 0 {
            (false_positives as f64 / total_alerts as f64) * 100.0
        } else {
            0.0
        };

        // Resolution time (hours between created_at and updated_at for resolved cases)
        let resolution_times: Vec<f64> = sqlx::query_scalar(
            "SELECT EXTRACT(EPOCH FROM (updated_at - created_at)) / 3600.0 
             FROM aml_cases 
             WHERE created_at >= $1 AND created_at < $2 
             AND status IN ('Cleared', 'PermanentlyBlocked')
             ORDER BY (updated_at - created_at)"
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_all(&self.pool)
        .await?;

        let avg_resolution_time_hrs = if !resolution_times.is_empty() {
            resolution_times.iter().sum::<f64>() / resolution_times.len() as f64
        } else {
            0.0
        };

        let median_resolution_time_hrs = if !resolution_times.is_empty() {
            let mid = resolution_times.len() / 2;
            resolution_times[mid]
        } else {
            0.0
        };

        // SLA breaches (cases taking > 24 hours to resolve)
        let sla_breaches: i64 = resolution_times.iter().filter(|&&t| t > 24.0).count() as i64;
        let resolved_count = resolution_times.len() as i64;
        let sla_compliance_rate = if resolved_count > 0 {
            ((resolved_count - sla_breaches) as f64 / resolved_count as f64) * 100.0
        } else {
            100.0
        };

        // Case disposition
        let cases_cleared: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aml_cases WHERE created_at >= $1 AND created_at < $2 AND status = 'Cleared'"
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_one(&self.pool)
        .await?;

        let cases_blocked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aml_cases WHERE created_at >= $1 AND created_at < $2 AND status = 'PermanentlyBlocked'"
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_one(&self.pool)
        .await?;

        let cases_pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aml_cases WHERE created_at >= $1 AND created_at < $2 AND status = 'PendingComplianceReview'"
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_one(&self.pool)
        .await?;

        // Risk distribution
        let low_risk_cases: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aml_cases WHERE created_at >= $1 AND created_at < $2 AND flag_level = 'LOW'"
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_one(&self.pool)
        .await?;

        let medium_risk_cases: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aml_cases WHERE created_at >= $1 AND created_at < $2 AND flag_level = 'MEDIUM'"
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_one(&self.pool)
        .await?;

        let critical_risk_cases: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aml_cases WHERE created_at >= $1 AND created_at < $2 AND flag_level = 'CRITICAL'"
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_one(&self.pool)
        .await?;

        // Trend analysis (compare to previous period)
        let period_duration = period_end.signed_duration_since(period_start);
        let prev_period_start = period_start - period_duration;
        let prev_period_end = period_start;

        let prev_total_alerts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aml_cases WHERE created_at >= $1 AND created_at < $2"
        )
        .bind(prev_period_start)
        .bind(prev_period_end)
        .fetch_one(&self.pool)
        .await?;

        let alert_volume_trend = if prev_total_alerts == 0 {
            None
        } else {
            let change = ((total_alerts as f64 - prev_total_alerts as f64) / prev_total_alerts as f64) * 100.0;
            Some(if change > 10.0 {
                TrendDirection::Increasing
            } else if change < -10.0 {
                TrendDirection::Decreasing
            } else {
                TrendDirection::Stable
            })
        };

        let prev_false_positives: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aml_cases 
             WHERE created_at >= $1 AND created_at < $2 
             AND status = 'Cleared' AND flag_level = 'LOW'"
        )
        .bind(prev_period_start)
        .bind(prev_period_end)
        .fetch_one(&self.pool)
        .await?;

        let prev_fp_rate = if prev_total_alerts > 0 {
            (prev_false_positives as f64 / prev_total_alerts as f64) * 100.0
        } else {
            0.0
        };

        let false_positive_trend = if prev_fp_rate == 0.0 {
            None
        } else {
            let change = ((false_positive_rate - prev_fp_rate) / prev_fp_rate) * 100.0;
            Some(if change > 10.0 {
                TrendDirection::Increasing
            } else if change < -10.0 {
                TrendDirection::Decreasing
            } else {
                TrendDirection::Stable
            })
        };

        Ok(ComplianceMetrics {
            period_start,
            period_end,
            total_alerts,
            sanctions_alerts,
            aml_alerts,
            kyc_alerts,
            false_positives,
            false_positive_rate,
            avg_resolution_time_hrs,
            median_resolution_time_hrs,
            sla_breaches,
            sla_compliance_rate,
            cases_cleared,
            cases_blocked,
            cases_pending,
            low_risk_cases,
            medium_risk_cases,
            critical_risk_cases,
            alert_volume_trend,
            false_positive_trend,
        })
    }

    // ── Report Persistence ────────────────────────────────────────────────────

    pub async fn save_report(
        &self,
        metrics: &ComplianceMetrics,
        report_type: &str,
        format: &str,
        generated_by: &str,
        file_path: Option<&str>,
    ) -> Result<ComplianceReport, anyhow::Error> {
        let report = sqlx::query_as::<_, ComplianceReport>(
            r#"
            INSERT INTO compliance_effectiveness_reports (
                report_type, period_start, period_end,
                total_alerts, sanctions_alerts, aml_alerts, kyc_alerts,
                false_positives, false_positive_rate,
                avg_resolution_time_hrs, median_resolution_time_hrs, sla_breaches, sla_compliance_rate,
                cases_cleared, cases_blocked, cases_pending,
                low_risk_cases, medium_risk_cases, critical_risk_cases,
                alert_volume_trend, false_positive_trend,
                generated_by, format, file_path
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24
            ) RETURNING *
            "#
        )
        .bind(report_type)
        .bind(metrics.period_start)
        .bind(metrics.period_end)
        .bind(metrics.total_alerts as i32)
        .bind(metrics.sanctions_alerts as i32)
        .bind(metrics.aml_alerts as i32)
        .bind(metrics.kyc_alerts as i32)
        .bind(metrics.false_positives as i32)
        .bind(metrics.false_positive_rate)
        .bind(metrics.avg_resolution_time_hrs)
        .bind(metrics.median_resolution_time_hrs)
        .bind(metrics.sla_breaches as i32)
        .bind(metrics.sla_compliance_rate)
        .bind(metrics.cases_cleared as i32)
        .bind(metrics.cases_blocked as i32)
        .bind(metrics.cases_pending as i32)
        .bind(metrics.low_risk_cases as i32)
        .bind(metrics.medium_risk_cases as i32)
        .bind(metrics.critical_risk_cases as i32)
        .bind(metrics.alert_volume_trend.as_ref().map(|t| t.to_string()))
        .bind(metrics.false_positive_trend.as_ref().map(|t| t.to_string()))
        .bind(generated_by)
        .bind(format)
        .bind(file_path)
        .fetch_one(&self.pool)
        .await?;

        Ok(report)
    }

    pub async fn list_reports(&self, query: &ListReportsQuery) -> Result<ReportListPage, anyhow::Error> {
        let mut sql = "SELECT * FROM compliance_effectiveness_reports WHERE 1=1".to_string();
        let mut count_sql = "SELECT COUNT(*) FROM compliance_effectiveness_reports WHERE 1=1".to_string();

        if let Some(ref rt) = query.report_type {
            sql.push_str(&format!(" AND report_type = '{rt}'"));
            count_sql.push_str(&format!(" AND report_type = '{rt}'"));
        }
        if let Some(from) = query.from {
            sql.push_str(&format!(" AND period_start >= '{from}'"));
            count_sql.push_str(&format!(" AND period_start >= '{from}'"));
        }
        if let Some(to) = query.to {
            sql.push_str(&format!(" AND period_end <= '{to}'"));
            count_sql.push_str(&format!(" AND period_end <= '{to}'"));
        }

        sql.push_str(" ORDER BY generated_at DESC");
        sql.push_str(&format!(" LIMIT {} OFFSET {}", query.page_size(), query.offset()));

        let reports: Vec<ComplianceReport> = sqlx::query_as(&sql).fetch_all(&self.pool).await?;
        let total: i64 = sqlx::query_scalar(&count_sql).fetch_one(&self.pool).await?;

        Ok(ReportListPage {
            reports,
            total,
            page: query.page(),
            page_size: query.page_size(),
        })
    }

    pub async fn get_report(&self, report_id: Uuid) -> Result<Option<ComplianceReport>, anyhow::Error> {
        let report = sqlx::query_as::<_, ComplianceReport>(
            "SELECT * FROM compliance_effectiveness_reports WHERE id = $1"
        )
        .bind(report_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(report)
    }

    // ── Audit Trail ───────────────────────────────────────────────────────────

    pub async fn log_report_access(
        &self,
        report_id: Uuid,
        action: &str,
        actor_id: &str,
        actor_role: &str,
        actor_ip: Option<&str>,
    ) -> Result<(), anyhow::Error> {
        sqlx::query(
            r#"
            INSERT INTO compliance_report_audit (report_id, action, actor_id, actor_role, actor_ip)
            VALUES ($1, $2, $3, $4, $5)
            "#
        )
        .bind(report_id)
        .bind(action)
        .bind(actor_id)
        .bind(actor_role)
        .bind(actor_ip)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // ── Schedules ─────────────────────────────────────────────────────────────

    pub async fn get_due_schedules(&self) -> Result<Vec<ReportSchedule>, anyhow::Error> {
        let schedules = sqlx::query_as::<_, ReportSchedule>(
            "SELECT * FROM compliance_report_schedules 
             WHERE enabled = TRUE AND (next_run_at IS NULL OR next_run_at <= NOW())
             ORDER BY next_run_at ASC"
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(schedules)
    }

    pub async fn update_schedule_run(
        &self,
        schedule_id: Uuid,
        next_run_at: DateTime<Utc>,
    ) -> Result<(), anyhow::Error> {
        sqlx::query(
            "UPDATE compliance_report_schedules 
             SET last_run_at = NOW(), next_run_at = $2 
             WHERE id = $1"
        )
        .bind(schedule_id)
        .bind(next_run_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
