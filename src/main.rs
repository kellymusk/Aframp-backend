use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aframp::{build_state, router, AppConfig};
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use axum::http::{header, HeaderValue, Method};
use tokio::sync::CancellationToken;
use tokio::sync::watch;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

/// Minimum poll interval (seconds) accepted for `STELLAR_POLL_INTERVAL_SECS`.
/// A very low value would hammer Horizon with requests.
const MIN_STELLAR_POLL_INTERVAL_SECS: u64 = 10;

/// Parse a numeric env var, failing fast on an unparseable value instead of
/// silently falling back to a default.
fn parse_env_u64(name: &str, default: u64) -> Result<u64, Box<dyn std::error::Error>> {
    match std::env::var(name) {
        Ok(raw) => raw.parse::<u64>().map_err(|e| {
            format!("invalid value for {name}: {raw:?} is not a valid u64 ({e})").into()
        }),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(e) => Err(format!("could not read {name}: {e}").into()),
    }
}

/// Resolve the Stellar poll interval, validating that it is set to a sane value.
fn stellar_poll_interval_secs() -> Result<u64, Box<dyn std::error::Error>> {
    let secs = parse_env_u64("STELLAR_POLL_INTERVAL_SECS", 60)?;
    if secs < MIN_STELLAR_POLL_INTERVAL_SECS {
        return Err(format!(
            "STELLAR_POLL_INTERVAL_SECS={secs} is too low; must be >= {MIN_STELLAR_POLL_INTERVAL_SECS} seconds"
        )
        .into());
    }
    Ok(secs)
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

async fn health_detailed(State(state): State<Arc<aframp::AppState>>) -> impl IntoResponse {
    let db_check = tokio::time::timeout(HEALTH_CHECK_TIMEOUT, sqlx::query("SELECT 1").execute(&state.db));
    let horizon_check = tokio::time::timeout(
        HEALTH_CHECK_TIMEOUT,
        reqwest::Client::new()
            .head(&state.config.stellar_horizon_url)
            .send(),
    );

    let (db_result, horizon_result) = tokio::join!(db_check, horizon_check);

    let database = match db_result {
        Ok(Ok(_)) => "ok",
        _ => "fail",
    };
    let horizon = match horizon_result {
        Ok(Ok(resp)) if resp.status().is_success() => "ok",
        _ => "fail",
    };

    let all_ok = database == "ok" && horizon == "ok";
    let status = if all_ok { "ok" } else { "fail" };
    let code = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let body = json!({
        "status": status,
        "checks": {
            "database": database,
            "horizon": horizon,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }
    });

    (code, Json(body))
// Single source of truth for the default request body limit (64KB); see
// `aframp::config` so tests and the binary can't drift apart.
use aframp::DEFAULT_MAX_REQUEST_BODY_BYTES;

/// Resolve the request body limit from `MAX_REQUEST_BODY_BYTES`, falling back
/// to [`DEFAULT_MAX_REQUEST_BODY_BYTES`] when unset or unparseable.
fn max_request_body_bytes() -> usize {
    match std::env::var("MAX_REQUEST_BODY_BYTES") {
        Ok(value) => match value.trim().parse::<usize>() {
            Ok(bytes) if bytes > 0 => bytes,
            _ => {
                tracing::warn!(
                    value = %value,
                    default = DEFAULT_MAX_REQUEST_BODY_BYTES,
                    "invalid MAX_REQUEST_BODY_BYTES, using default"
                );
                DEFAULT_MAX_REQUEST_BODY_BYTES
            }
        },
        Err(_) => DEFAULT_MAX_REQUEST_BODY_BYTES,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info".into());

    // LOG_FORMAT=json emits structured JSON logs for production log aggregators
    // (Datadog, CloudWatch, etc.); anything else keeps human-readable text for dev.
    let log_format = std::env::var("LOG_FORMAT").unwrap_or_else(|_| "text".to_string());
    if log_format.eq_ignore_ascii_case("json") {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(env_filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }

    let config = AppConfig::from_env()?;
    let poll_interval_secs = stellar_poll_interval_secs()?;
    let state = Arc::new(build_state(&config).await?);

    let shutdown = CancellationToken::new();
    // Channel used to signal the blockchain worker to stop on shutdown.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let worker = aframp::blockchain::worker::run(
        state.clone(),
        config.stellar_horizon_url.clone(),
        poll_interval_secs,
        config.stellar_poll_interval_secs,
        shutdown.clone(),
    );
    let worker_handle = tokio::spawn(worker);

    // Hard-delete expired+cancelled payment requests older than 30 days.
    let cleanup_state = state.clone();
    tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(60 * 60);
        loop {
            match aframp::services::payment_requests::hard_delete_expired_cancelled(
                &cleanup_state.db,
            )
            .await
            {
                Ok(n) if n > 0 => {
                    tracing::info!(deleted = n, "hard-deleted expired cancelled payment requests")
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(error = %err, "payment request cleanup failed")
                }
            }
            tokio::time::sleep(interval).await;
        }
    });
    // Background OTP cleanup: deletes challenges older than 24 hours past
    // expiry every hour. Prevents unbounded growth of the otp_challenges table.
    // See services::otp::cleanup_expired for the retention policy.
    {
        let db = state.db.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600));
            loop {
                interval.tick().await;
                match aframp::services::otp::cleanup_expired(&db).await {
                    Ok(n) => tracing::info!(deleted = n, "otp_challenges cleanup complete"),
                    Err(e) => tracing::error!(error = %e, "otp_challenges cleanup failed"),
                }
            }
        });
    }

    // Auth travels as an HttpOnly cookie for browsers, so credentials are on —
    // which means origins must be listed explicitly, never mirrored back.
    let origins = config
        .cors_allowed_origins
        .iter()
        .map(|origin| origin.parse::<HeaderValue>())
        .collect::<Result<Vec<_>, _>>()?;
    tracing::info!(?origins, "cors allowed origins");

    if config.cookie.same_site == aframp::SameSite::None {
        tracing::warn!(
            "COOKIE_SAME_SITE=none sends the session on cross-site requests; \
             serve the frontend same-origin instead if you can, or add CSRF tokens"
        );
    }

    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    let max_request_body_bytes = max_request_body_bytes();
    tracing::info!(max_request_body_bytes, "request body limit configured");

    let app = router((*state).clone())
        .route("/health/detailed", get(health_detailed))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(max_request_body_bytes));

    let address: SocketAddr = config.bind_addr.parse()?;
    tracing::info!(%address, "aframp started");
    axum::serve(
        tokio::net::TcpListener::bind(address).await?,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    let server_shutdown = shutdown.clone();
    axum::serve(tokio::net::TcpListener::bind(address).await?, app)
        .with_graceful_shutdown(shutdown_signal(server_shutdown))
        .await?;

    // Ensure the worker observes the cancellation and exits cleanly.
    shutdown.cancel();
    if let Err(err) = worker_handle.await {
        tracing::error!(%err, "blockchain worker task failed to join");
    }

    Ok(())
}

/// Resolves when the process receives SIGTERM or Ctrl-C, then cancels the
/// shared token so the blockchain worker can terminate gracefully.
async fn shutdown_signal(token: CancellationToken) {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(%err, "failed to install Ctrl-C handler");
        }
    tracing::info!("blockchain worker running");

    let shutdown_tx = shutdown_tx.clone();
    let server = axum::serve(tokio::net::TcpListener::bind(address).await?, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_tx));

    // Run the server and the worker concurrently; if the worker panics, surface
    // the JoinError to the main task instead of silently swallowing it.
    tokio::select! {
        result = server => {
            result?;
        }
        result = worker_handle => {
            result??;
        }
    }

    // Signal the worker to stop and await its JoinHandle on shutdown.
    let _ = shutdown_rx;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_poll_interval_is_rejected() {
        std::env::set_var("STELLAR_POLL_INTERVAL_SECS", "abc");
        let result = stellar_poll_interval_secs();
        std::env::remove_var("STELLAR_POLL_INTERVAL_SECS");
        let err = result.expect_err("unparseable poll interval must be an error");
        assert!(err.to_string().contains("STELLAR_POLL_INTERVAL_SECS"));
    }

    #[test]
    fn too_low_poll_interval_is_rejected() {
        std::env::set_var("STELLAR_POLL_INTERVAL_SECS", "1");
        let result = stellar_poll_interval_secs();
        std::env::remove_var("STELLAR_POLL_INTERVAL_SECS");
        assert!(result.is_err());
    }

    #[test]
    fn valid_poll_interval_is_accepted() {
        std::env::set_var("STELLAR_POLL_INTERVAL_SECS", "30");
        let result = stellar_poll_interval_secs();
        std::env::remove_var("STELLAR_POLL_INTERVAL_SECS");
        assert_eq!(result.unwrap(), 30);
    }
async fn shutdown_signal(shutdown_tx: watch::Sender<bool>) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                tracing::error!(%err, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }

    tracing::info!("shutdown signal received, cancelling blockchain worker");
    token.cancel();
    tracing::info!("shutdown signal received, stopping worker");
    let _ = shutdown_tx.send(true);
}
