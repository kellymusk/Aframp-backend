use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub environment: String,
    pub stellar_network: String,
    pub afri_configured: bool,
}

pub async fn health_check(
    State(config): State<Config>,
) -> Result<Json<HealthResponse>, StatusCode> {
    let version = env!("CARGO_PKG_VERSION").to_string();

    let afri_configured = !config.afri.asset_code.is_empty()
        && !config.afri.issuer_address.is_empty()
        && !config.afri.supported_currencies.is_empty();

    let response = HealthResponse {
        status: "healthy".to_string(),
        version,
        environment: config.server.environment.clone(),
        stellar_network: config.stellar.network.clone(),
        afri_configured,
    };

    Ok(Json(response))
}
