use super::models::*;
use crate::database::PgPool;
use chrono::Utc;
use std::sync::Arc;
use uuid::Uuid;

/// Minimum score (0-100) required to unlock production access.
const PRODUCTION_GATE_SCORE: i16 = 80;

/// A single certification test definition.
struct CertTest {
    name: &'static str,
    category: &'static str,
    /// Returns (passed, error_message)
    run: fn(&CertContext) -> (bool, Option<String>),
}

/// Lightweight context passed to each test function.
struct CertContext {
    has_test_users: bool,
    has_bank_accounts: bool,
    has_completed_onramp: bool,
    has_completed_offramp: bool,
    has_stellar_address: bool,
    has_webhook_config: bool,
    has_oauth_client: bool,
    has_api_key: bool,
    has_error_handling_test: bool,
}

static CERT_TESTS: &[CertTest] = &[
    CertTest {
        name: "test_data_generated",
        category: "deposit",
        run: |ctx| (ctx.has_test_users, if !ctx.has_test_users { Some("No test users found. Call POST /sandbox/data/generate first.".into()) } else { None }),
    },
    CertTest {
        name: "bank_account_linked",
        category: "deposit",
        run: |ctx| (ctx.has_bank_accounts, if !ctx.has_bank_accounts { Some("No test bank accounts found.".into()) } else { None }),
    },
    CertTest {
        name: "successful_deposit",
        category: "deposit",
        run: |ctx| (ctx.has_completed_onramp, if !ctx.has_completed_onramp { Some("No completed onramp transaction found.".into()) } else { None }),
    },
    CertTest {
        name: "successful_withdrawal",
        category: "withdrawal",
        run: |ctx| (ctx.has_completed_offramp, if !ctx.has_completed_offramp { Some("No completed offramp transaction found.".into()) } else { None }),
    },
    CertTest {
        name: "balance_inquiry",
        category: "balance",
        run: |ctx| (ctx.has_test_users, if !ctx.has_test_users { Some("No test users to query balance for.".into()) } else { None }),
    },
    CertTest {
        name: "stellar_testnet_address",
        category: "balance",
        run: |ctx| (ctx.has_stellar_address, if !ctx.has_stellar_address { Some("No Stellar testnet address provisioned.".into()) } else { None }),
    },
    CertTest {
        name: "webhook_endpoint_configured",
        category: "webhook",
        run: |ctx| (ctx.has_webhook_config, if !ctx.has_webhook_config { Some("No webhook endpoint configured.".into()) } else { None }),
    },
    CertTest {
        name: "oauth2_client_registered",
        category: "oauth",
        run: |ctx| (ctx.has_oauth_client, if !ctx.has_oauth_client { Some("No OAuth2 client registered.".into()) } else { None }),
    },
    CertTest {
        name: "api_key_provisioned",
        category: "oauth",
        run: |ctx| (ctx.has_api_key, if !ctx.has_api_key { Some("No API key provisioned.".into()) } else { None }),
    },
    CertTest {
        name: "error_handling_scenario_tested",
        category: "error_handling",
        run: |ctx| (ctx.has_error_handling_test, if !ctx.has_error_handling_test { Some("No chaos scenario has been activated. Test your error handling.".into()) } else { None }),
    },
];

#[derive(Clone)]
pub struct CertificationService {
    pool: Arc<PgPool>,
}

impl CertificationService {
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }

    pub async fn run_certification(
        &self,
        application_id: Uuid,
    ) -> Result<CertificationRunSummary, DeveloperPortalError> {
        // Gather context from DB
        let ctx = self.build_context(application_id).await?;

        // Create run record
        let run = sqlx::query_as!(
            SandboxCertificationRun,
            r#"INSERT INTO sandbox_certification_runs
               (application_id, status, passed_tests, total_tests)
               VALUES ($1, 'running', 0, $2)
               RETURNING *"#,
            application_id,
            CERT_TESTS.len() as i16,
        )
        .fetch_one(self.pool.as_ref())
        .await?;

        // Execute each test
        let mut results = Vec::with_capacity(CERT_TESTS.len());
        let mut passed = 0i16;

        for test in CERT_TESTS {
            let start = std::time::Instant::now();
            let (test_passed, error_message) = (test.run)(&ctx);
            let duration_ms = start.elapsed().as_millis() as i32;

            if test_passed {
                passed += 1;
            }

            let result = sqlx::query_as!(
                SandboxCertificationResult,
                r#"INSERT INTO sandbox_certification_results
                   (run_id, test_name, category, passed, error_message, duration_ms)
                   VALUES ($1, $2, $3, $4, $5, $6)
                   RETURNING *"#,
                run.id,
                test.name,
                test.category,
                test_passed,
                error_message,
                duration_ms,
            )
            .fetch_one(self.pool.as_ref())
            .await?;

            results.push(result);
        }

        let total = CERT_TESTS.len() as i16;
        let score = (passed * 100 / total) as i16;
        let gate_met = score >= PRODUCTION_GATE_SCORE;
        let status = if gate_met { "passed" } else { "failed" };

        let run = sqlx::query_as!(
            SandboxCertificationRun,
            r#"UPDATE sandbox_certification_runs
               SET status = $1, score = $2, passed_tests = $3,
                   production_gate_met = $4, completed_at = now()
               WHERE id = $5
               RETURNING *"#,
            status,
            score,
            passed,
            gate_met,
            run.id,
        )
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(CertificationRunSummary { run, results })
    }

    pub async fn latest_run(
        &self,
        application_id: Uuid,
    ) -> Result<Option<CertificationRunSummary>, DeveloperPortalError> {
        let run = sqlx::query_as!(
            SandboxCertificationRun,
            "SELECT * FROM sandbox_certification_runs WHERE application_id = $1 ORDER BY started_at DESC LIMIT 1",
            application_id
        )
        .fetch_optional(self.pool.as_ref())
        .await?;

        let Some(run) = run else { return Ok(None) };

        let results = sqlx::query_as!(
            SandboxCertificationResult,
            "SELECT * FROM sandbox_certification_results WHERE run_id = $1 ORDER BY executed_at",
            run.id
        )
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(Some(CertificationRunSummary { run, results }))
    }

    /// Returns true if the application has passed certification and may request production access.
    pub async fn production_gate_met(&self, application_id: Uuid) -> Result<bool, DeveloperPortalError> {
        let row = sqlx::query!(
            r#"SELECT production_gate_met FROM sandbox_certification_runs
               WHERE application_id = $1
               ORDER BY started_at DESC LIMIT 1"#,
            application_id
        )
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(row.map(|r| r.production_gate_met).unwrap_or(false))
    }

    // ── Context builder ───────────────────────────────────────────────────────

    async fn build_context(&self, application_id: Uuid) -> Result<CertContext, DeveloperPortalError> {
        let user_count: i64 = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM sandbox_test_users WHERE application_id = $1",
            application_id
        )
        .fetch_one(self.pool.as_ref())
        .await?
        .unwrap_or(0);

        let bank_count: i64 = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM sandbox_test_bank_accounts WHERE application_id = $1",
            application_id
        )
        .fetch_one(self.pool.as_ref())
        .await?
        .unwrap_or(0);

        let onramp_count: i64 = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM sandbox_mock_transactions WHERE application_id = $1 AND transaction_type = 'onramp' AND status = 'completed'",
            application_id
        )
        .fetch_one(self.pool.as_ref())
        .await?
        .unwrap_or(0);

        let offramp_count: i64 = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM sandbox_mock_transactions WHERE application_id = $1 AND transaction_type = 'offramp' AND status = 'completed'",
            application_id
        )
        .fetch_one(self.pool.as_ref())
        .await?
        .unwrap_or(0);

        let stellar_count: i64 = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM sandbox_test_users WHERE application_id = $1 AND stellar_address IS NOT NULL",
            application_id
        )
        .fetch_one(self.pool.as_ref())
        .await?
        .unwrap_or(0);

        let webhook_count: i64 = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM webhook_configurations WHERE application_id = $1 AND status = 'active'",
            application_id
        )
        .fetch_one(self.pool.as_ref())
        .await?
        .unwrap_or(0);

        let oauth_count: i64 = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM oauth_clients WHERE application_id = $1 AND environment = 'sandbox' AND status = 'active'",
            application_id
        )
        .fetch_one(self.pool.as_ref())
        .await?
        .unwrap_or(0);

        let api_key_count: i64 = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM api_keys WHERE application_id = $1 AND environment = 'sandbox' AND status = 'active'",
            application_id
        )
        .fetch_one(self.pool.as_ref())
        .await?
        .unwrap_or(0);

        let chaos_count: i64 = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM sandbox_chaos_scenarios WHERE application_id = $1 AND activated_at IS NOT NULL",
            application_id
        )
        .fetch_one(self.pool.as_ref())
        .await?
        .unwrap_or(0);

        Ok(CertContext {
            has_test_users: user_count > 0,
            has_bank_accounts: bank_count > 0,
            has_completed_onramp: onramp_count > 0,
            has_completed_offramp: offramp_count > 0,
            has_stellar_address: stellar_count > 0,
            has_webhook_config: webhook_count > 0,
            has_oauth_client: oauth_count > 0,
            has_api_key: api_key_count > 0,
            has_error_handling_test: chaos_count > 0,
        })
    }
}
