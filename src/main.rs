use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aframp::{build_state, router, AppConfig};
use axum::http::{header, HeaderValue, Method};
use sqlx::PgPool;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

/// OTP audit retention policy: challenges are kept for 24 hours past their
/// expiry so that recently verified/expired OTPs remain auditable, then the
/// scheduled cleanup below deletes them. This keeps `otp_challenges` bounded
/// even though consumed rows are marked with `consumed_at` rather than deleted
/// inline on successful verification.
const OTP_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const OTP_CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Periodically deletes OTP challenges that expired more than `OTP_RETENTION`
/// ago. Uses the `otp_challenges(expires_at)` index for an efficient range scan.
async fn run_otp_cleanup(pool: PgPool) {
    let mut ticker = tokio::time::interval(OTP_CLEANUP_INTERVAL);
    loop {
        ticker.tick().await;
        let cutoff = chrono::Utc::now()
            - chrono::Duration::from_std(OTP_RETENTION).unwrap_or_default();
        match sqlx::query("DELETE FROM otp_challenges WHERE expires_at < $1")
            .bind(cutoff)
            .execute(&pool)
            .await
        {
            Ok(result) => {
                if result.rows_affected() > 0 {
                    tracing::info!(
                        deleted = result.rows_affected(),
                        "pruned expired otp_challenges"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(%error, "otp_challenges cleanup failed");
            }
        }
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

    // Keep otp_challenges bounded: consumed rows are retained for audit and
    // pruned once they are 24 hours past expiry.
    tokio::spawn(run_otp_cleanup(state.db.clone()));

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

    let app = router((*state).clone())
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(1024 * 1024));

    let address: SocketAddr = config.bind_addr.parse()?;
    tracing::info!(%address, "aframp started");
    axum::serve(tokio::net::TcpListener::bind(address).await?, app).await?;
    Ok(())
}
