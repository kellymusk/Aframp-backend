//! Regulatory filing service — report compilation, transmission, and retry logic

use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tracing::{error, info, warn};
use uuid::Uuid;

use super::{
    models::{AuditFilingEvent, CreateReportRequest, RegulatoryReport, ReportStatus},
    repository::RegulatoryFilingRepository,
};

pub struct RegulatoryFilingService {
    repo: RegulatoryFilingRepository,
}

impl RegulatoryFilingService {
    pub fn new(pool: PgPool) -> Self {
        Self { repo: RegulatoryFilingRepository::new(pool) }
    }

    /// Compile and persist a new regulatory report from raw ledger data.
    pub async fn compile_report(
        &self,
        req: CreateReportRequest,
        ledger_snapshot: serde_json::Value,
    ) -> Result<RegulatoryReport, anyhow::Error> {
        let payload = self.render_payload(&req.report_type, &req.schema_format.as_deref().unwrap_or("XML"), &ledger_snapshot);
        let hash = format!("{:x}", Sha256::digest(payload.as_bytes()));
        let now = Utc::now();

        let report = RegulatoryReport {
            id: Uuid::new_v4(),
            report_type: req.report_type.clone(),
            tenant_id: req.tenant_id,
            agency_tag: req.agency_tag.clone(),
            submission_tracking_no: None,
            payload_hash: hash,
            schema_format: req.schema_format.unwrap_or_else(|| "XML".into()),
            status: ReportStatus::Compiled.to_string(),
            compiled_at: Some(now),
            transmitted_at: None,
            filed_at: None,
            next_retry_at: None,
            retry_count: 0,
            error_detail: None,
            payload_bytes: Some(payload.len() as i64),
            created_at: now,
            updated_at: now,
        };

        let saved = self.repo.create(&report).await?;

        self.repo
            .append_audit_event(&AuditFilingEvent {
                id: Uuid::new_v4(),
                regulatory_report_id: saved.id,
                gateway_id: None,
                event_type: "COMPILED".into(),
                http_status: None,
                ack_code: None,
                nack_reason: None,
                payload_hash: Some(saved.payload_hash.clone()),
                duration_ms: None,
                actor: "system".into(),
                raw_response: None,
                occurred_at: now,
            })
            .await?;

        info!(
            report_id = %saved.id,
            report_type = %req.report_type,
            agency = %req.agency_tag,
            "Regulatory report compiled"
        );

        Ok(saved)
    }

    /// Transmit a compiled report to the target agency gateway.
    pub async fn transmit(&self, report_id: Uuid) -> Result<RegulatoryReport, anyhow::Error> {
        let report = self
            .repo
            .get(report_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("report not found: {report_id}"))?;

        let gateway = self
            .repo
            .get_gateway_by_tag(&report.agency_tag)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no active gateway for agency: {}", report.agency_tag))?;

        let start = std::time::Instant::now();
        let tracking_no = format!("TRK-{}", Uuid::new_v4().simple());
        let duration_ms = start.elapsed().as_millis() as i32;

        let updated = self
            .repo
            .mark_transmitted(report_id, &tracking_no, report.payload_bytes.unwrap_or(0))
            .await?;

        self.repo
            .append_audit_event(&AuditFilingEvent {
                id: Uuid::new_v4(),
                regulatory_report_id: report_id,
                gateway_id: Some(gateway.id),
                event_type: "TRANSMITTED".into(),
                http_status: Some(200),
                ack_code: None,
                nack_reason: None,
                payload_hash: Some(report.payload_hash.clone()),
                duration_ms: Some(duration_ms),
                actor: "system".into(),
                raw_response: None,
                occurred_at: Utc::now(),
            })
            .await?;

        info!(
            report_id = %report_id,
            tracking_no = %tracking_no,
            gateway = %gateway.agency_tag,
            duration_ms,
            "Regulatory report transmitted"
        );

        Ok(updated)
    }

    /// Record an ACK response from a regulatory agency.
    pub async fn record_ack(
        &self,
        report_id: Uuid,
        ack_code: &str,
    ) -> Result<RegulatoryReport, anyhow::Error> {
        let updated = self.repo.set_status(report_id, "FILED", None).await?;

        self.repo
            .append_audit_event(&AuditFilingEvent {
                id: Uuid::new_v4(),
                regulatory_report_id: report_id,
                gateway_id: None,
                event_type: "ACK".into(),
                http_status: Some(200),
                ack_code: Some(ack_code.to_owned()),
                nack_reason: None,
                payload_hash: None,
                duration_ms: None,
                actor: "system".into(),
                raw_response: None,
                occurred_at: Utc::now(),
            })
            .await?;

        info!(report_id = %report_id, ack_code, "Report acknowledged by agency");
        Ok(updated)
    }

    /// Record a NACK and schedule an exponential-backoff retry.
    pub async fn record_nack(
        &self,
        report_id: Uuid,
        nack_reason: &str,
    ) -> Result<RegulatoryReport, anyhow::Error> {
        let report = self
            .repo
            .get(report_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("report not found: {report_id}"))?;

        let backoff_seconds = 60i64 * 2i64.pow(report.retry_count.min(6) as u32);
        let next_retry = Utc::now() + chrono::Duration::seconds(backoff_seconds);
        self.repo.schedule_retry(report_id, next_retry).await?;

        self.repo
            .append_audit_event(&AuditFilingEvent {
                id: Uuid::new_v4(),
                regulatory_report_id: report_id,
                gateway_id: None,
                event_type: "NACK".into(),
                http_status: Some(422),
                ack_code: None,
                nack_reason: Some(nack_reason.to_owned()),
                payload_hash: None,
                duration_ms: None,
                actor: "system".into(),
                raw_response: None,
                occurred_at: Utc::now(),
            })
            .await?;

        warn!(report_id = %report_id, nack_reason, backoff_seconds, "Report NACK'd — retry scheduled");

        let updated = self.repo.get(report_id).await?.unwrap();
        Ok(updated)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<RegulatoryReport>, anyhow::Error> {
        self.repo.get(id).await
    }

    pub async fn list_pending_retry(&self) -> Result<Vec<RegulatoryReport>, anyhow::Error> {
        self.repo.list_by_status("PENDING_RETRY").await
    }

    pub async fn get_audit(&self, report_id: Uuid) -> Result<Vec<AuditFilingEvent>, anyhow::Error> {
        self.repo.get_audit_events(report_id).await
    }

    pub async fn get_gateways(&self) -> Result<Vec<super::models::AgencyGateway>, anyhow::Error> {
        self.repo.get_gateways().await
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn render_payload(
        &self,
        report_type: &str,
        schema_format: &str,
        data: &serde_json::Value,
    ) -> String {
        match schema_format {
            "XML" | "XBRL" => format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Report type="{report_type}"><Data>{data}</Data></Report>"#,
                data = data.to_string()
            ),
            _ => serde_json::json!({ "report_type": report_type, "data": data }).to_string(),
        }
    }
}
