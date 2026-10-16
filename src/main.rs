use std::net::SocketAddr;
use std::sync::Arc;

use aframp::{build_state, router, AppConfig};
use axum::http::{header, HeaderValue, Method};
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

/// Default maximum request body size (1MB) used when `MAX_REQUEST_BODY_BYTES`
/// is not set. This is a request body limit, not a response body limit.
const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;

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
    let state = Arc::new(build_state(&config).await?);

    let listener = aframp::blockchain::worker::run(
        state.clone(),
        config.stellar_horizon_url.clone(),
        config.stellar_poll_interval_secs,
    );
    tokio::spawn(listener);

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
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(max_request_body_bytes));

    let address: SocketAddr = config.bind_addr.parse()?;
    tracing::info!(%address, "aframp started");
    axum::serve(tokio::net::TcpListener::bind(address).await?, app).await?;
    Ok(())
}
