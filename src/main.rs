use std::net::SocketAddr;
use std::sync::Arc;

use aframp::{build_state, router, AppConfig};
use axum::http::{header, HeaderName, HeaderValue, Method, Request};
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

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
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    let request_id_header = HeaderName::from_static("x-request-id");

    let app = router((*state).clone())
        .layer(cors)
        // Order matters: set before Trace so the id is in scope for the span,
        // propagate after Trace so it lands on the response headers Trace
        // itself doesn't touch.
        .layer(SetRequestIdLayer::new(
            request_id_header.clone(),
            aframp::middleware::SanitizingRequestId,
        ))
        .layer(
            TraceLayer::new_for_http().make_span_with(move |req: &Request<axum::body::Body>| {
                let request_id = req
                    .headers()
                    .get(&request_id_header)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                tracing::info_span!(
                    "request",
                    method = %req.method(),
                    uri = %req.uri(),
                    request_id = %request_id,
                )
            }),
        )
        .layer(PropagateRequestIdLayer::new(HeaderName::from_static(
            "x-request-id",
        )))
        .layer(RequestBodyLimitLayer::new(1024 * 1024));

    let address: SocketAddr = config.bind_addr.parse()?;
    tracing::info!(%address, "aframp started");
    axum::serve(tokio::net::TcpListener::bind(address).await?, app).await?;
    Ok(())
}
