//! Compliance Effectiveness Report Generation Service
//!
//! Generates compliance health reports in PDF, CSV, and JSON formats.
//! PDF is produced via a Typst template; CSV and JSON are generated in-memory.

use super::models::{ComplianceMetrics, ComplianceReport, ReportFormat, ReportType};
use super::repository::ComplianceEffectivenessRepository;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tracing::{error, info};
use uuid::Uuid;

pub struct ReportGenerationService {
    repo: Arc<ComplianceEffectivenessRepository>,
}

impl ReportGenerationService {
    pub fn new(repo: Arc<ComplianceEffectivenessRepository>) -> Self {
        Self { repo }
    }

    /// Generate a compliance effectiveness report for the given period.
    /// Aggregates KPIs, serialises to the requested format, and persists metadata.
    pub async fn generate(
        &self,
        report_type: &ReportType,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
        format: &ReportFormat,
        generated_by: &str,
    ) -> Result<GeneratedReport, anyhow::Error> {
        info!(
            report_type = %report_type,
            period_start = %period_start,
            period_end = %period_end,
            format = %format,
            generated_by = %generated_by,
            "Generating compliance effectiveness report"
        );

        let metrics = self.repo.aggregate_metrics(period_start, period_end).await?;

        let (content, content_type) = match format {
            ReportFormat::Json => {
                let json = serde_json::to_string_pretty(&metrics)?;
                (json.into_bytes(), "application/json")
            }
            ReportFormat::Csv => {
                let csv = render_csv(&metrics);
                (csv.into_bytes(), "text/csv")
            }
            ReportFormat::Pdf => {
                let pdf = render_pdf(&metrics, report_type, period_start, period_end)?;
                (pdf, "application/pdf")
            }
        };

        let report = self
            .repo
            .save_report(&metrics, &report_type.to_string(), &format.to_string(), generated_by, None)
            .await?;

        info!(
            report_id = %report.id,
            bytes = content.len(),
            "Compliance report generated successfully"
        );

        Ok(GeneratedReport {
            report,
            content,
            content_type,
        })
    }
}

// ── Output ────────────────────────────────────────────────────────────────────

pub struct GeneratedReport {
    pub report: ComplianceReport,
    pub content: Vec<u8>,
    pub content_type: &'static str,
}

// ── CSV Renderer ──────────────────────────────────────────────────────────────

fn render_csv(m: &ComplianceMetrics) -> String {
    let mut w = csv::Writer::from_writer(vec![]);

    // Header row
    w.write_record(&[
        "metric", "value", "unit",
    ]).ok();

    let rows: &[(&str, String, &str)] = &[
        ("period_start",             m.period_start.to_rfc3339(),                    "datetime"),
        ("period_end",               m.period_end.to_rfc3339(),                      "datetime"),
        ("total_alerts",             m.total_alerts.to_string(),                     "count"),
        ("sanctions_alerts",         m.sanctions_alerts.to_string(),                 "count"),
        ("aml_alerts",               m.aml_alerts.to_string(),                       "count"),
        ("kyc_alerts",               m.kyc_alerts.to_string(),                       "count"),
        ("false_positives",          m.false_positives.to_string(),                  "count"),
        ("false_positive_rate",      format!("{:.2}", m.false_positive_rate),        "percent"),
        ("avg_resolution_time_hrs",  format!("{:.2}", m.avg_resolution_time_hrs),    "hours"),
        ("median_resolution_time_hrs", format!("{:.2}", m.median_resolution_time_hrs), "hours"),
        ("sla_breaches",             m.sla_breaches.to_string(),                     "count"),
        ("sla_compliance_rate",      format!("{:.2}", m.sla_compliance_rate),        "percent"),
        ("cases_cleared",            m.cases_cleared.to_string(),                    "count"),
        ("cases_blocked",            m.cases_blocked.to_string(),                    "count"),
        ("cases_pending",            m.cases_pending.to_string(),                    "count"),
        ("low_risk_cases",           m.low_risk_cases.to_string(),                   "count"),
        ("medium_risk_cases",        m.medium_risk_cases.to_string(),                "count"),
        ("critical_risk_cases",      m.critical_risk_cases.to_string(),              "count"),
        ("alert_volume_trend",       m.alert_volume_trend.as_ref().map(|t| t.to_string()).unwrap_or_default(), "trend"),
        ("false_positive_trend",     m.false_positive_trend.as_ref().map(|t| t.to_string()).unwrap_or_default(), "trend"),
    ];

    for (metric, value, unit) in rows {
        w.write_record(&[metric, value, unit]).ok();
    }

    String::from_utf8(w.into_inner().unwrap_or_default()).unwrap_or_default()
}

// ── PDF Renderer (Typst) ──────────────────────────────────────────────────────

fn render_pdf(
    m: &ComplianceMetrics,
    report_type: &ReportType,
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
) -> Result<Vec<u8>, anyhow::Error> {
    let typst_source = build_typst_source(m, report_type, period_start, period_end);

    // Compile via typst
    let world = TypstStringWorld::new(typst_source);
    let document = typst::compile(&world)
        .map_err(|e| anyhow::anyhow!("Typst compilation failed: {:?}", e))?;

    // Export to PDF using typst's built-in export
    let pdf_bytes = typst::export::pdf(&document)
        .map_err(|e| anyhow::anyhow!("PDF export failed: {:?}", e))?;

    Ok(pdf_bytes)
}

fn build_typst_source(
    m: &ComplianceMetrics,
    report_type: &ReportType,
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
) -> String {
    let trend_alert = m.alert_volume_trend.as_ref().map(|t| t.to_string()).unwrap_or_else(|| "N/A".to_string());
    let trend_fp = m.false_positive_trend.as_ref().map(|t| t.to_string()).unwrap_or_else(|| "N/A".to_string());

    format!(
        r#"
#set page(paper: "a4", margin: (x: 2cm, y: 2.5cm))
#set text(font: "Liberation Sans", size: 10pt)
#set heading(numbering: "1.")

#align(center)[
  #text(size: 18pt, weight: "bold")[Aframp AML/KYC Compliance Effectiveness Report]
  #linebreak()
  #text(size: 12pt)[{report_type} Report — {period_start} to {period_end}]
  #linebreak()
  #text(size: 9pt, fill: gray)[Generated: {generated_at} | CONFIDENTIAL — For Regulatory Use Only]
]

#line(length: 100%)

= Executive Summary

This report summarises the compliance programme effectiveness for the period *{period_start}* to *{period_end}*.

#table(
  columns: (auto, auto),
  stroke: 0.5pt,
  fill: (col, row) => if row == 0 {{ luma(220) }} else {{ white }},
  [*Metric*], [*Value*],
  [Total Compliance Alerts], [{total_alerts}],
  [False Positive Rate], [{fp_rate:.2}%],
  [SLA Compliance Rate], [{sla_rate:.2}%],
  [Average Resolution Time], [{avg_res:.2} hours],
  [Cases Pending Review], [{cases_pending}],
)

= Alert Volume Analysis

#table(
  columns: (auto, auto, auto),
  stroke: 0.5pt,
  fill: (col, row) => if row == 0 {{ luma(220) }} else {{ white }},
  [*Alert Type*], [*Count*], [*% of Total*],
  [Sanctions Screening], [{sanctions}], [{sanctions_pct:.1}%],
  [AML / Transaction Monitoring], [{aml}], [{aml_pct:.1}%],
  [KYC / Corridor Risk], [{kyc}], [{kyc_pct:.1}%],
)

*Alert Volume Trend (vs prior period):* {trend_alert}

= False Positive Analysis

- Total False Positives: *{fp_count}*
- False Positive Rate: *{fp_rate:.2}%*
- Trend: *{trend_fp}*

A false positive is defined as a case flagged at LOW risk level that was subsequently cleared by a compliance officer.

= SLA Compliance

- Average Resolution Time: *{avg_res:.2} hours*
- Median Resolution Time: *{med_res:.2} hours*
- SLA Breaches (> 24 hours): *{sla_breaches}*
- SLA Compliance Rate: *{sla_rate:.2}%*

= Case Disposition

#table(
  columns: (auto, auto),
  stroke: 0.5pt,
  fill: (col, row) => if row == 0 {{ luma(220) }} else {{ white }},
  [*Status*], [*Count*],
  [Cleared], [{cleared}],
  [Permanently Blocked], [{blocked}],
  [Pending Review], [{cases_pending}],
)

= Risk Distribution

#table(
  columns: (auto, auto),
  stroke: 0.5pt,
  fill: (col, row) => if row == 0 {{ luma(220) }} else {{ white }},
  [*Risk Level*], [*Count*],
  [Low], [{low_risk}],
  [Medium], [{med_risk}],
  [Critical], [{crit_risk}],
)

#line(length: 100%)
#text(size: 8pt, fill: gray)[
  This report is generated automatically by the Aframp Compliance Effectiveness Reporting System.
  It is intended for use by authorised compliance personnel and regulators (CBN/NFIU) only.
  Unauthorised disclosure is prohibited.
]
"#,
        report_type = report_type,
        period_start = period_start.format("%Y-%m-%d"),
        period_end = period_end.format("%Y-%m-%d"),
        generated_at = Utc::now().format("%Y-%m-%d %H:%M UTC"),
        total_alerts = m.total_alerts,
        fp_rate = m.false_positive_rate,
        sla_rate = m.sla_compliance_rate,
        avg_res = m.avg_resolution_time_hrs,
        cases_pending = m.cases_pending,
        sanctions = m.sanctions_alerts,
        sanctions_pct = if m.total_alerts > 0 { m.sanctions_alerts as f64 / m.total_alerts as f64 * 100.0 } else { 0.0 },
        aml = m.aml_alerts,
        aml_pct = if m.total_alerts > 0 { m.aml_alerts as f64 / m.total_alerts as f64 * 100.0 } else { 0.0 },
        kyc = m.kyc_alerts,
        kyc_pct = if m.total_alerts > 0 { m.kyc_alerts as f64 / m.total_alerts as f64 * 100.0 } else { 0.0 },
        trend_alert = trend_alert,
        fp_count = m.false_positives,
        trend_fp = trend_fp,
        med_res = m.median_resolution_time_hrs,
        sla_breaches = m.sla_breaches,
        cleared = m.cases_cleared,
        blocked = m.cases_blocked,
        low_risk = m.low_risk_cases,
        med_risk = m.medium_risk_cases,
        crit_risk = m.critical_risk_cases,
    )
}

// ── Minimal Typst World ───────────────────────────────────────────────────────

struct TypstStringWorld {
    source: String,
    library: std::sync::OnceLock<std::sync::Arc<typst::foundations::Library>>,
    book: std::sync::OnceLock<std::sync::Arc<typst::text::FontBook>>,
    fonts: Vec<typst::text::Font>,
    time: time::OffsetDateTime,
}

impl TypstStringWorld {
    fn new(source: String) -> Self {
        let time = time::OffsetDateTime::now_utc();
        Self {
            source,
            library: std::sync::OnceLock::new(),
            book: std::sync::OnceLock::new(),
            fonts: load_system_fonts(),
            time,
        }
    }
}

impl typst::World for TypstStringWorld {
    fn library(&self) -> &std::sync::Arc<typst::foundations::Library> {
        self.library.get_or_init(|| std::sync::Arc::new(typst::foundations::Library::default()))
    }

    fn book(&self) -> &std::sync::Arc<typst::text::FontBook> {
        self.book.get_or_init(|| {
            let mut book = typst::text::FontBook::new();
            for font in &self.fonts {
                book.push(font.info().clone());
            }
            std::sync::Arc::new(book)
        })
    }

    fn main(&self) -> typst::syntax::FileId {
        typst::syntax::FileId::new(None, typst::syntax::VirtualPath::new("main.typ"))
    }

    fn source(&self, id: typst::syntax::FileId) -> typst::diag::FileResult<typst::syntax::Source> {
        if id == self.main() {
            Ok(typst::syntax::Source::new(id, self.source.clone()))
        } else {
            Err(typst::diag::FileError::NotFound(std::path::PathBuf::from("not found")))
        }
    }

    fn file(&self, _id: typst::syntax::FileId) -> typst::diag::FileResult<typst::foundations::Bytes> {
        Err(typst::diag::FileError::NotFound(std::path::PathBuf::from("not found")))
    }

    fn font(&self, index: usize) -> Option<typst::text::Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, offset: Option<i64>) -> Option<typst::foundations::Datetime> {
        let offset = offset.unwrap_or(0);
        let dt = self.time + time::Duration::hours(offset);
        typst::foundations::Datetime::from_ymd(
            dt.year(),
            dt.month() as u8,
            dt.day(),
        )
    }
}

fn load_system_fonts() -> Vec<typst::text::Font> {
    let mut fonts = Vec::new();
    let paths = [
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    ];
    for path in &paths {
        if let Ok(data) = std::fs::read(path) {
            let bytes = typst::foundations::Bytes::from(data);
            for font in typst::text::Font::iter(bytes) {
                fonts.push(font);
            }
        }
    }
    fonts
}
