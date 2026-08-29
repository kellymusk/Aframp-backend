//! Corporate Registry Service
//!
//! Integrates with official government registries to verify business entities.
//! Primary: CAC (Corporate Affairs Commission) Nigeria API.
//! Fallback: Mock provider for development / unsupported jurisdictions.

use super::models::{RegistryEntityData, ShareholderRecord};
use async_trait::async_trait;
use tracing::{info, warn};

// ── Trait ─────────────────────────────────────────────────────────────────────

#[async_trait]
pub trait RegistryProvider: Send + Sync {
    async fn lookup(&self, registration_number: &str) -> Result<RegistryEntityData, RegistryError>;
    fn provider_name(&self) -> &'static str;
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("Entity not found: {0}")]
    NotFound(String),
    #[error("Registry API error: {0}")]
    ApiError(String),
    #[error("Invalid registration number format")]
    InvalidFormat,
}

// ── CAC Nigeria Provider ──────────────────────────────────────────────────────

pub struct CacNigeriaProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl CacNigeriaProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: std::env::var("CAC_API_URL")
                .unwrap_or_else(|_| "https://api.cac.gov.ng/v1".to_string()),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl RegistryProvider for CacNigeriaProvider {
    async fn lookup(&self, registration_number: &str) -> Result<RegistryEntityData, RegistryError> {
        info!(reg_number = %registration_number, "CAC Nigeria registry lookup");

        let url = format!("{}/company/{}", self.base_url, registration_number);
        let resp = self
            .client
            .get(&url)
            .header("x-api-key", &self.api_key)
            .send()
            .await
            .map_err(|e| RegistryError::ApiError(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(RegistryError::NotFound(registration_number.to_string()));
        }
        if !resp.status().is_success() {
            return Err(RegistryError::ApiError(format!("HTTP {}", resp.status())));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| RegistryError::ApiError(e.to_string()))?;

        parse_cac_response(&body)
    }

    fn provider_name(&self) -> &'static str {
        "cac_nigeria"
    }
}

fn parse_cac_response(body: &serde_json::Value) -> Result<RegistryEntityData, RegistryError> {
    let status_raw = body["status"].as_str().unwrap_or("unknown").to_lowercase();
    let status = match status_raw.as_str() {
        "active" | "registered" => "active",
        "inactive" | "suspended" => "inactive",
        "deregistered" | "dissolved" | "struck off" => "deregistered",
        _ => "unknown",
    };

    let shareholders = body["shareholders"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|s| {
            Some(ShareholderRecord {
                name: s["name"].as_str()?.to_string(),
                ownership_percentage: s["percentage"].as_f64().unwrap_or(0.0),
            })
        })
        .collect();

    let directors = body["directors"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|d| d["name"].as_str().map(|s| s.to_string()))
        .collect();

    Ok(RegistryEntityData {
        registration_number: body["rcNumber"].as_str().unwrap_or("").to_string(),
        company_name: body["companyName"].as_str().unwrap_or("").to_string(),
        status: status.to_string(),
        registered_address: body["address"].as_str().map(|s| s.to_string()),
        incorporation_date: body["dateOfIncorporation"]
            .as_str()
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()),
        directors,
        shareholders,
    })
}

// ── Mock Provider (dev / unsupported jurisdictions) ───────────────────────────

pub struct MockRegistryProvider;

#[async_trait]
impl RegistryProvider for MockRegistryProvider {
    async fn lookup(&self, registration_number: &str) -> Result<RegistryEntityData, RegistryError> {
        warn!(reg_number = %registration_number, "Using mock registry provider");

        // Simulate inactive/deregistered for specific test numbers
        if registration_number.starts_with("INACTIVE") {
            return Ok(RegistryEntityData {
                registration_number: registration_number.to_string(),
                company_name: "Inactive Corp Ltd".to_string(),
                status: "inactive".to_string(),
                registered_address: None,
                incorporation_date: None,
                directors: vec![],
                shareholders: vec![],
            });
        }
        if registration_number.starts_with("DEREG") {
            return Ok(RegistryEntityData {
                registration_number: registration_number.to_string(),
                company_name: "Deregistered Corp Ltd".to_string(),
                status: "deregistered".to_string(),
                registered_address: None,
                incorporation_date: None,
                directors: vec![],
                shareholders: vec![],
            });
        }

        Ok(RegistryEntityData {
            registration_number: registration_number.to_string(),
            company_name: format!("Mock Company {}", registration_number),
            status: "active".to_string(),
            registered_address: Some("123 Mock Street, Lagos, Nigeria".to_string()),
            incorporation_date: Some(chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap()),
            directors: vec!["John Doe".to_string()],
            shareholders: vec![
                ShareholderRecord { name: "John Doe".to_string(), ownership_percentage: 60.0 },
                ShareholderRecord { name: "Jane Smith".to_string(), ownership_percentage: 40.0 },
            ],
        })
    }

    fn provider_name(&self) -> &'static str {
        "mock"
    }
}

// ── Factory ───────────────────────────────────────────────────────────────────

pub fn registry_provider_for(jurisdiction: &str) -> Box<dyn RegistryProvider> {
    match jurisdiction.to_uppercase().as_str() {
        "NG" => {
            let api_key = std::env::var("CAC_API_KEY").unwrap_or_default();
            if api_key.is_empty() {
                warn!("CAC_API_KEY not set — using mock registry provider");
                Box::new(MockRegistryProvider)
            } else {
                Box::new(CacNigeriaProvider::new(api_key))
            }
        }
        _ => Box::new(MockRegistryProvider),
    }
}
