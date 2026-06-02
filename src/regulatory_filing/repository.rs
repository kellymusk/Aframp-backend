//! Database repository for the regulatory filing pipeline

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use super::models::{AgencyGateway, AuditFilingEvent, RegulatoryReport};

pub struct RegulatoryFilingRepository {
    pool: PgPool,
}

impl RegulatoryFilingRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, report: &RegulatoryReport) -> Result<RegulatoryReport, anyhow::Error> {
        Ok(sqlx::query_as!(
            RegulatoryReport,
            r#"
            INSERT INTO regulatory_reports
                (id, report_type, tenant_id, agency_tag, payload_hash, schema_format, status,
                 retry_count, created_at, updated_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,0,$8,$8)
            RETURNING *
            "#,
            report.id,
            report.report_type,
            report.tenant_id,
            report.agency_tag,
            report.payload_hash,
            report.schema_format,
            report.status,
            report.created_at,
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<RegulatoryReport>, anyhow::Error> {
        Ok(sqlx::query_as!(
            RegulatoryReport,
            "SELECT * FROM regulatory_reports WHERE id = $1",
            id
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn list_by_status(&self, status: &str) -> Result<Vec<RegulatoryReport>, anyhow::Error> {
        Ok(sqlx::query_as!(
            RegulatoryReport,
            "SELECT * FROM regulatory_reports WHERE status = $1 ORDER BY created_at DESC LIMIT 200",
            status
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn set_status(
        &self,
        id: Uuid,
        status: &str,
        error_detail: Option<&str>,
    ) -> Result<RegulatoryReport, anyhow::Error> {
        Ok(sqlx::query_as!(
            RegulatoryReport,
            r#"
            UPDATE regulatory_reports
            SET status = $2, error_detail = $3, updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
            id,
            status,
            error_detail,
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn mark_transmitted(
        &self,
        id: Uuid,
        tracking_no: &str,
        payload_bytes: i64,
    ) -> Result<RegulatoryReport, anyhow::Error> {
        Ok(sqlx::query_as!(
            RegulatoryReport,
            r#"
            UPDATE regulatory_reports
            SET status = 'TRANSMITTED',
                submission_tracking_no = $2,
                transmitted_at = NOW(),
                payload_bytes = $3,
                updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
            id,
            tracking_no,
            payload_bytes,
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn schedule_retry(
        &self,
        id: Uuid,
        next_retry_at: chrono::DateTime<Utc>,
    ) -> Result<(), anyhow::Error> {
        sqlx::query!(
            r#"
            UPDATE regulatory_reports
            SET status = 'PENDING_RETRY',
                next_retry_at = $2,
                retry_count = retry_count + 1,
                updated_at = NOW()
            WHERE id = $1
            "#,
            id,
            next_retry_at,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_gateways(&self) -> Result<Vec<AgencyGateway>, anyhow::Error> {
        Ok(sqlx::query_as!(
            AgencyGateway,
            "SELECT * FROM agency_gateways WHERE is_active = TRUE ORDER BY agency_tag"
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_gateway_by_tag(&self, tag: &str) -> Result<Option<AgencyGateway>, anyhow::Error> {
        Ok(sqlx::query_as!(
            AgencyGateway,
            "SELECT * FROM agency_gateways WHERE agency_tag = $1 AND is_active = TRUE",
            tag
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn append_audit_event(&self, ev: &AuditFilingEvent) -> Result<(), anyhow::Error> {
        sqlx::query!(
            r#"
            INSERT INTO audit_filing_history
                (id, regulatory_report_id, gateway_id, event_type, http_status,
                 ack_code, nack_reason, payload_hash, duration_ms, actor, raw_response, occurred_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            "#,
            ev.id,
            ev.regulatory_report_id,
            ev.gateway_id,
            ev.event_type,
            ev.http_status,
            ev.ack_code,
            ev.nack_reason,
            ev.payload_hash,
            ev.duration_ms,
            ev.actor,
            ev.raw_response,
            ev.occurred_at,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_audit_events(
        &self,
        report_id: Uuid,
    ) -> Result<Vec<AuditFilingEvent>, anyhow::Error> {
        Ok(sqlx::query_as!(
            AuditFilingEvent,
            r#"
            SELECT id, regulatory_report_id, gateway_id, event_type, http_status,
                   ack_code, nack_reason, payload_hash, duration_ms, actor, raw_response, occurred_at
            FROM audit_filing_history
            WHERE regulatory_report_id = $1
            ORDER BY occurred_at ASC
            "#,
            report_id,
        )
        .fetch_all(&self.pool)
        .await?)
    }
}
