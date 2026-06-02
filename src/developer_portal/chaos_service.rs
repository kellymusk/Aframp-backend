use super::models::*;
use crate::database::PgPool;
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct ChaosInjectionService {
    pool: Arc<PgPool>,
}

impl ChaosInjectionService {
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }

    pub async fn create_scenario(
        &self,
        application_id: Uuid,
        req: CreateChaosScenarioRequest,
    ) -> Result<SandboxChaosScenario, DeveloperPortalError> {
        let valid_types = ["http_500", "http_429", "tx_rejected", "latency_ms", "network_timeout"];
        if !valid_types.contains(&req.scenario_type.as_str()) {
            return Err(DeveloperPortalError::InvalidStatus);
        }

        let config = req.config.unwrap_or_else(|| self.default_config(&req.scenario_type));
        let target = req.target_path_prefix.unwrap_or_else(|| "/".to_string());
        let expires_at = req
            .expires_in_secs
            .map(|s| Utc::now() + chrono::Duration::seconds(s as i64));

        let scenario = sqlx::query_as!(
            SandboxChaosScenario,
            r#"INSERT INTO sandbox_chaos_scenarios
               (application_id, scenario_type, config, target_path_prefix, is_active, expires_at)
               VALUES ($1, $2, $3, $4, false, $5)
               RETURNING *"#,
            application_id,
            req.scenario_type,
            config,
            target,
            expires_at,
        )
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(scenario)
    }

    pub async fn set_active(
        &self,
        application_id: Uuid,
        scenario_id: Uuid,
        active: bool,
    ) -> Result<SandboxChaosScenario, DeveloperPortalError> {
        let activated_at = if active { Some(Utc::now()) } else { None };

        let scenario = sqlx::query_as!(
            SandboxChaosScenario,
            r#"UPDATE sandbox_chaos_scenarios
               SET is_active = $1, activated_at = $2
               WHERE id = $3 AND application_id = $4
               RETURNING *"#,
            active,
            activated_at,
            scenario_id,
            application_id,
        )
        .fetch_optional(self.pool.as_ref())
        .await?
        .ok_or(DeveloperPortalError::ApplicationNotFound)?;

        Ok(scenario)
    }

    pub async fn list_scenarios(
        &self,
        application_id: Uuid,
    ) -> Result<Vec<SandboxChaosScenario>, DeveloperPortalError> {
        let scenarios = sqlx::query_as!(
            SandboxChaosScenario,
            "SELECT * FROM sandbox_chaos_scenarios WHERE application_id = $1 ORDER BY created_at DESC",
            application_id
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(scenarios)
    }

    pub async fn delete_scenario(
        &self,
        application_id: Uuid,
        scenario_id: Uuid,
    ) -> Result<(), DeveloperPortalError> {
        let rows = sqlx::query!(
            "DELETE FROM sandbox_chaos_scenarios WHERE id = $1 AND application_id = $2",
            scenario_id,
            application_id
        )
        .execute(self.pool.as_ref())
        .await?
        .rows_affected();

        if rows == 0 {
            return Err(DeveloperPortalError::ApplicationNotFound);
        }
        Ok(())
    }

    /// Returns the active scenario for a given path, if any (and not expired).
    pub async fn active_scenario_for_path(
        &self,
        application_id: Uuid,
        path: &str,
    ) -> Result<Option<SandboxChaosScenario>, DeveloperPortalError> {
        let scenario = sqlx::query_as!(
            SandboxChaosScenario,
            r#"SELECT * FROM sandbox_chaos_scenarios
               WHERE application_id = $1
                 AND is_active = true
                 AND (expires_at IS NULL OR expires_at > now())
                 AND $2 LIKE (target_path_prefix || '%')
               ORDER BY created_at DESC
               LIMIT 1"#,
            application_id,
            path,
        )
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(scenario)
    }

    fn default_config(&self, scenario_type: &str) -> serde_json::Value {
        match scenario_type {
            "latency_ms" => json!({"delay_ms": 2000}),
            "http_429" => json!({"retry_after": 60}),
            "tx_rejected" => json!({"reason": "INSUFFICIENT_FUNDS"}),
            _ => json!({}),
        }
    }
}
