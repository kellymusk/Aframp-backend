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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = AppConfig::from_env()?;
    let state = Arc::new(build_state(&config).await?);

    let listener = aframp::blockchain::worker::run(
        state.clone(),
        config.stellar_horizon_url.clone(),
        config.stellar_poll_interval_secs,
    );
    tokio::spawn(listener);

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
