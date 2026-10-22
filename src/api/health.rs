use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use crate::AppState;

/// Per-dependency probe budget, so a hung dependency can't hang the check.
const CHECK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Serialize)]
pub struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

/// Liveness probe. Returns 200 with a small JSON body (not a bodiless 204),
/// which Cloudflare's container health checks expect.
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Serialize)]
pub struct DetailedHealth {
    status: &'static str,
    version: &'static str,
    checks: Checks,
}

#[derive(Serialize)]
struct Checks {
    database: &'static str,
    horizon: &'static str,
}

/// Readiness probe: checks the database and Horizon concurrently, each within
/// [`CHECK_TIMEOUT`]. 200 when both answer, 503 (with the per-check result)
/// otherwise. Kept separate from [`health`] so a Horizon outage doesn't make
/// the platform restart a healthy container.
pub async fn detailed(State(state): State<AppState>) -> (StatusCode, Json<DetailedHealth>) {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let http = HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(CHECK_TIMEOUT)
            .build()
            .expect("failed to build health-check HTTP client")
    });

    let database = tokio::time::timeout(CHECK_TIMEOUT, sqlx::query("SELECT 1").execute(&state.db));
    let horizon = tokio::time::timeout(CHECK_TIMEOUT, http.get(state.stellar_horizon_url.as_str()).send());
    let (database, horizon) = tokio::join!(database, horizon);

    let database = if matches!(database, Ok(Ok(_))) { "ok" } else { "fail" };
    let horizon = match horizon {
        Ok(Ok(resp)) if resp.status().is_success() => "ok",
        _ => "fail",
    };
    let healthy = database == "ok" && horizon == "ok";
    (
        if healthy { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE },
        Json(DetailedHealth {
            status: if healthy { "ok" } else { "degraded" },
            version: env!("CARGO_PKG_VERSION"),
            checks: Checks { database, horizon },
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_returns_ok_status_and_version() {
        let Json(body) = health().await;
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
    }
}
