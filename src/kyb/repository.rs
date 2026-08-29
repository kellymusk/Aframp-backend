//! KYB Repository — Database persistence layer

use super::models::{KybApplication, KybDocument, KybRiskScore, RegistryCheck, Ubo};
use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

pub struct KybRepository {
    pool: PgPool,
}

impl KybRepository {
    pub fn new(pool: PgPool) -> Self { Self { pool } }
    pub fn pool(&self) -> &PgPool { &self.pool }

    // ── Applications ──────────────────────────────────────────────────────────

    pub async fn create_application(
        &self,
        merchant_id: Uuid,
        business_name: &str,
        registration_number: &str,
        jurisdiction: &str,
        industry_code: Option<&str>,
    ) -> Result<KybApplication, anyhow::Error> {
        Ok(sqlx::query_as::<_, KybApplication>(
            r#"INSERT INTO kyb_applications
               (merchant_id, business_name, registration_number, jurisdiction, industry_code)
               VALUES ($1, $2, $3, $4, $5) RETURNING *"#,
        )
        .bind(merchant_id).bind(business_name).bind(registration_number)
        .bind(jurisdiction).bind(industry_code)
        .fetch_one(&self.pool).await?)
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<KybApplication>, anyhow::Error> {
        Ok(sqlx::query_as::<_, KybApplication>(
            "SELECT * FROM kyb_applications WHERE id = $1"
        ).bind(id).fetch_optional(&self.pool).await?)
    }

    pub async fn get_by_merchant(&self, merchant_id: Uuid) -> Result<Option<KybApplication>, anyhow::Error> {
        Ok(sqlx::query_as::<_, KybApplication>(
            "SELECT * FROM kyb_applications WHERE merchant_id = $1"
        ).bind(merchant_id).fetch_optional(&self.pool).await?)
    }

    pub async fn transition_status(
        &self,
        id: Uuid,
        new_status: &str,
    ) -> Result<KybApplication, anyhow::Error> {
        Ok(sqlx::query_as::<_, KybApplication>(
            "UPDATE kyb_applications SET status = $2 WHERE id = $1 RETURNING *"
        ).bind(id).bind(new_status).fetch_one(&self.pool).await?)
    }

    pub async fn set_registry_result(
        &self,
        id: Uuid,
        registry_status: &str,
        registry_data: serde_json::Value,
        registered_address: Option<&str>,
    ) -> Result<(), anyhow::Error> {
        sqlx::query(
            r#"UPDATE kyb_applications
               SET registry_status = $2, registry_data = $3, registered_address = $4,
                   registry_verified_at = NOW()
               WHERE id = $1"#,
        )
        .bind(id).bind(registry_status).bind(registry_data).bind(registered_address)
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn set_risk_score(
        &self,
        id: Uuid,
        score: f64,
        risk_level: &str,
    ) -> Result<(), anyhow::Error> {
        sqlx::query(
            "UPDATE kyb_applications SET risk_score = $2, risk_level = $3 WHERE id = $1"
        ).bind(id).bind(score).bind(risk_level).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn set_review_decision(
        &self,
        id: Uuid,
        approved: bool,
        reviewer: &str,
        notes: Option<&str>,
        rejection_reason: Option<&str>,
    ) -> Result<KybApplication, anyhow::Error> {
        let status = if approved { "approved" } else { "rejected" };
        let approved_at = if approved { Some(Utc::now()) } else { None };
        Ok(sqlx::query_as::<_, KybApplication>(
            r#"UPDATE kyb_applications
               SET status = $2, reviewed_by = $3, review_notes = $4,
                   rejection_reason = $5, approved_at = $6
               WHERE id = $1 RETURNING *"#,
        )
        .bind(id).bind(status).bind(reviewer).bind(notes)
        .bind(rejection_reason).bind(approved_at)
        .fetch_one(&self.pool).await?)
    }

    // ── UBOs ──────────────────────────────────────────────────────────────────

    pub async fn upsert_ubo(
        &self,
        kyb_application_id: Uuid,
        full_name: &str,
        ownership_percentage: f64,
    ) -> Result<Ubo, anyhow::Error> {
        Ok(sqlx::query_as::<_, Ubo>(
            r#"INSERT INTO kyb_ubos (kyb_application_id, full_name, ownership_percentage)
               VALUES ($1, $2, $3)
               ON CONFLICT DO NOTHING
               RETURNING *"#,
        )
        .bind(kyb_application_id).bind(full_name).bind(ownership_percentage)
        .fetch_one(&self.pool).await?)
    }

    pub async fn set_ubo_kyc_status(&self, ubo_id: Uuid, status: &str) -> Result<(), anyhow::Error> {
        sqlx::query("UPDATE kyb_ubos SET kyc_status = $2 WHERE id = $1")
            .bind(ubo_id).bind(status).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn list_ubos(&self, kyb_application_id: Uuid) -> Result<Vec<Ubo>, anyhow::Error> {
        Ok(sqlx::query_as::<_, Ubo>(
            "SELECT * FROM kyb_ubos WHERE kyb_application_id = $1"
        ).bind(kyb_application_id).fetch_all(&self.pool).await?)
    }

    // ── Documents ─────────────────────────────────────────────────────────────

    pub async fn save_document(
        &self,
        kyb_application_id: Uuid,
        document_type: &str,
        file_name: &str,
        file_path: &str,
        file_hash: &str,
        ocr_data: Option<serde_json::Value>,
        ocr_confidence: Option<f64>,
    ) -> Result<KybDocument, anyhow::Error> {
        Ok(sqlx::query_as::<_, KybDocument>(
            r#"INSERT INTO kyb_documents
               (kyb_application_id, document_type, file_name, file_path, file_hash,
                ocr_extracted_data, ocr_confidence)
               VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *"#,
        )
        .bind(kyb_application_id).bind(document_type).bind(file_name)
        .bind(file_path).bind(file_hash).bind(ocr_data).bind(ocr_confidence)
        .fetch_one(&self.pool).await?)
    }

    pub async fn list_documents(&self, kyb_application_id: Uuid) -> Result<Vec<KybDocument>, anyhow::Error> {
        Ok(sqlx::query_as::<_, KybDocument>(
            "SELECT * FROM kyb_documents WHERE kyb_application_id = $1"
        ).bind(kyb_application_id).fetch_all(&self.pool).await?)
    }

    // ── Registry Checks ───────────────────────────────────────────────────────

    pub async fn save_registry_check(
        &self,
        kyb_application_id: Uuid,
        provider: &str,
        registration_number: &str,
        check_status: &str,
        response_data: Option<serde_json::Value>,
        error_message: Option<&str>,
    ) -> Result<RegistryCheck, anyhow::Error> {
        Ok(sqlx::query_as::<_, RegistryCheck>(
            r#"INSERT INTO kyb_registry_checks
               (kyb_application_id, registry_provider, registration_number,
                check_status, response_data, error_message)
               VALUES ($1, $2, $3, $4, $5, $6) RETURNING *"#,
        )
        .bind(kyb_application_id).bind(provider).bind(registration_number)
        .bind(check_status).bind(response_data).bind(error_message)
        .fetch_one(&self.pool).await?)
    }

    // ── Risk Scores ───────────────────────────────────────────────────────────

    pub async fn save_risk_score(
        &self,
        kyb_application_id: Uuid,
        score: f64,
        risk_level: &str,
        factors: serde_json::Value,
    ) -> Result<KybRiskScore, anyhow::Error> {
        Ok(sqlx::query_as::<_, KybRiskScore>(
            r#"INSERT INTO kyb_risk_scores (kyb_application_id, score, risk_level, factors)
               VALUES ($1, $2, $3, $4) RETURNING *"#,
        )
        .bind(kyb_application_id).bind(score).bind(risk_level).bind(factors)
        .fetch_one(&self.pool).await?)
    }

    pub async fn latest_risk_score(&self, kyb_application_id: Uuid) -> Result<Option<KybRiskScore>, anyhow::Error> {
        Ok(sqlx::query_as::<_, KybRiskScore>(
            "SELECT * FROM kyb_risk_scores WHERE kyb_application_id = $1 ORDER BY calculated_at DESC LIMIT 1"
        ).bind(kyb_application_id).fetch_optional(&self.pool).await?)
    }
}
