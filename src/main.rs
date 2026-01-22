mod api;
mod config;

use axum::{routing::get, Router};
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment variables from .env file
    dotenv::dotenv().ok();

    // Initialize logging
    tracing_subscriber::fmt::init();

    // Load configuration
    let config = config::Config::from_env()?;

    // Log startup info
    tracing::info!("Starting Aframp Backend");
    tracing::info!("Environment: {}", config.server.environment);
    tracing::info!("Stellar Network: {}", config.stellar.network);
    tracing::info!("AFRI Asset: {}", config.afri.asset_code);

    // Build router
    let app = Router::new()
        .route("/health", get(api::health::health_check))
        .with_state(config.clone());

    // Start server
    let addr = SocketAddr::from(([0, 0, 0, 0], config.server.port));
    tracing::info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
