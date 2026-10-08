use std::net::SocketAddr;
use std::sync::Arc;

use aframp::{build_state, router, AppConfig};
use axum::http::{HeaderName, HeaderValue, Request};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

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

    // `--rotate-key` runs the WALLET_ENCRYPTION_KEY rotation and exits —
    // it never starts the HTTP server or the deposit worker. See
    // src/rotate_key.rs for the full operational sequence.
    if std::env::args().nth(1).as_deref() == Some("--rotate-key") {
        return aframp::rotate_key::run().await;
    }

    let config = AppConfig::from_env()?;
    let state = Arc::new(build_state(&config).await?);

    // Reconcile pending withdrawals on startup (older than 10 minutes)
    if let Err(err) = aframp::services::withdrawals::reconcile_pending_withdrawals(
        &state.db,
        state.payment_provider.as_ref(),
    )
    .await
    {
        tracing::error!(error = %err, "failed to reconcile pending withdrawals on startup");
    }

    let listener = aframp::blockchain::worker::run(
        state.clone(),
        config.stellar_horizon_url.clone(),
        config.stellar_poll_interval_secs,
        config.stellar_poll_concurrency,
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
    // See services::otp::purge_stale_challenges for the retention policy.
    {
        let db = state.db.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600));
            loop {
                interval.tick().await;
                match aframp::services::otp::purge_stale_challenges(&db).await {
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

    let cors = aframp::cors_layer(origins);

    // Parsed and validated in AppConfig::from_env (invalid values fail startup).
    let max_request_body_bytes = config.max_request_body_bytes;
    tracing::info!(max_request_body_bytes, "request body limit configured");

    let app = router((*state).clone())
        .layer(cors)
        // X-Request-ID: set before Trace so the id is on the request span,
        // propagated after so it lands on the response. Incoming ids are
        // only trusted if they're valid UUIDs (see SanitizingRequestId).
        .layer(SetRequestIdLayer::new(
            HeaderName::from_static("x-request-id"),
            aframp::middleware::SanitizingRequestId,
        ))
        .layer(TraceLayer::new_for_http().make_span_with(|req: &Request<axum::body::Body>| {
            let request_id = req
                .headers()
                .get("x-request-id")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            tracing::info_span!(
                "request",
                method = %req.method(),
                uri = %req.uri(),
                request_id = %request_id,
            )
        }))
        .layer(PropagateRequestIdLayer::new(HeaderName::from_static("x-request-id")))
        .layer(RequestBodyLimitLayer::new(max_request_body_bytes));

    let address: SocketAddr = config.bind_addr.parse()?;
    tracing::info!(%address, "aframp started");
    axum::serve(tokio::net::TcpListener::bind(address).await?, app).await?;
    Ok(())
}
