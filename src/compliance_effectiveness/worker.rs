//! Compliance Effectiveness Reporting Worker
//!
//! Polls `compliance_report_schedules` every minute, generates due reports,
//! and advances `next_run_at` based on the cron expression.

use super::models::ReportFormat;
use super::repository::ComplianceEffectivenessRepository;
use super::service::ReportGenerationService;
use chrono::{DateTime, Datelike, Duration, Utc};
use std::sync::Arc;
use tokio::time::{interval, Duration as TokioDuration};
use tracing::{error, info, warn};

pub struct ComplianceReportWorker {
    service: Arc<ReportGenerationService>,
    repo: Arc<ComplianceEffectivenessRepository>,
}

impl ComplianceReportWorker {
    pub fn new(
        service: Arc<ReportGenerationService>,
        repo: Arc<ComplianceEffectivenessRepository>,
    ) -> Self {
        Self { service, repo }
    }

    /// Spawn the worker as a background task. Polls every 60 seconds.
    pub fn start(self) {
        tokio::spawn(async move {
            let mut ticker = interval(TokioDuration::from_secs(60));
            loop {
                ticker.tick().await;
                self.run_due_schedules().await;
            }
        });
    }

    async fn run_due_schedules(&self) {
        let schedules = match self.repo.get_due_schedules().await {
            Ok(s) => s,
            Err(e) => {
                error!(error = %e, "Failed to fetch due compliance report schedules");
                return;
            }
        };

        for schedule in schedules {
            info!(
                schedule = %schedule.schedule_name,
                report_type = %schedule.report_type,
                "Running due compliance report schedule"
            );

            let (period_start, period_end) = period_for_type(&schedule.report_type);
            let format = parse_format(&schedule.format);

            match self
                .service
                .generate(
                    &schedule.report_type.parse().unwrap_or(super::models::ReportType::Monthly),
                    period_start,
                    period_end,
                    &format,
                    &format!("scheduler:{}", schedule.schedule_name),
                )
                .await
            {
                Ok(generated) => {
                    info!(
                        report_id = %generated.report.id,
                        schedule = %schedule.schedule_name,
                        "Scheduled compliance report generated"
                    );

                    // Log audit event
                    if let Err(e) = self
                        .repo
                        .log_report_access(
                            generated.report.id,
                            "generated",
                            &format!("scheduler:{}", schedule.schedule_name),
                            "system",
                            None,
                        )
                        .await
                    {
                        warn!(error = %e, "Failed to log scheduled report audit event");
                    }
                }
                Err(e) => {
                    error!(
                        schedule = %schedule.schedule_name,
                        error = %e,
                        "Scheduled compliance report generation failed"
                    );
                }
            }

            // Advance next_run_at
            let next = next_run_from_cron(&schedule.cron_expression);
            if let Err(e) = self.repo.update_schedule_run(schedule.id, next).await {
                error!(
                    schedule = %schedule.schedule_name,
                    error = %e,
                    "Failed to update schedule next_run_at"
                );
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Compute the reporting period for a given report type (previous complete period).
fn period_for_type(report_type: &str) -> (DateTime<Utc>, DateTime<Utc>) {
    let now = Utc::now();
    match report_type {
        "monthly" => {
            // Previous calendar month
            let end = now
                .with_day(1)
                .unwrap_or(now)
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .map(|dt| dt.and_utc())
                .unwrap_or(now);
            let start = (end - Duration::days(1))
                .with_day(1)
                .unwrap_or(end - Duration::days(31));
            (start, end)
        }
        "quarterly" => {
            // Previous calendar quarter
            let month = now.month();
            let quarter_start_month = ((month - 1) / 3) * 3 + 1;
            let end = now
                .with_month(quarter_start_month)
                .and_then(|d| d.with_day(1))
                .unwrap_or(now)
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .map(|dt| dt.and_utc())
                .unwrap_or(now);
            let start = end - Duration::days(91);
            (start, end)
        }
        "annual" => {
            // Previous calendar year
            let end = now
                .with_month(1)
                .and_then(|d| d.with_day(1))
                .unwrap_or(now)
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .map(|dt| dt.and_utc())
                .unwrap_or(now);
            let start = end - Duration::days(365);
            (start, end)
        }
        _ => {
            // Default: last 30 days
            let end = now;
            let start = end - Duration::days(30);
            (start, end)
        }
    }
}

/// Advance next_run_at based on a simplified cron expression.
/// Supports: "0 0 1 * *" (monthly), "0 0 1 1,4,7,10 *" (quarterly), "0 0 1 1 *" (annual).
fn next_run_from_cron(cron: &str) -> DateTime<Utc> {
    let now = Utc::now();
    let parts: Vec<&str> = cron.split_whitespace().collect();
    if parts.len() < 5 {
        return now + Duration::days(30);
    }

    // Detect cadence from month field
    let month_field = parts[3];
    if month_field.contains(',') {
        // Quarterly: advance ~3 months
        now + Duration::days(91)
    } else if month_field == "1" && parts[4] == "*" {
        // Annual
        now + Duration::days(365)
    } else {
        // Monthly
        now + Duration::days(30)
    }
}

fn parse_format(s: &str) -> ReportFormat {
    match s {
        "csv" => ReportFormat::Csv,
        "json" => ReportFormat::Json,
        _ => ReportFormat::Pdf,
    }
}
