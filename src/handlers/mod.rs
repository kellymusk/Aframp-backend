use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::AppState;

/// Assets for which we expose NGN rates.
const SUPPORTED_ASSETS: &[&str] = &["XLM", "USDC", "NGNT"];

/// How long a cached rates snapshot stays fresh.
const RATES_CACHE_TTL: Duration = Duration::from_secs(60);

/// Source label reported alongside the rates.
const RATES_SOURCE: &str = "aframp";

#[derive(Debug, Clone, Serialize)]
pub struct AssetRate {
    pub asset: String,
    pub ngn: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RatesResponse {
    /// Unix timestamp (seconds) when the rates were produced.
    pub timestamp: i64,
    /// Where the rates came from.
    pub source: String,
    pub rates: Vec<AssetRate>,
}

struct CachedRates {
    fetched_at: Instant,
    response: RatesResponse,
}

/// Server-side cache for the rates snapshot.
pub struct RatesCache {
    inner: RwLock<Option<CachedRates>>,
}

impl RatesCache {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(None),
        }
    }

    async fn get_or_refresh(&self) -> RatesResponse {
        {
            let guard = self.inner.read().await;
            if let Some(cached) = guard.as_ref() {
                if cached.fetched_at.elapsed() < RATES_CACHE_TTL {
                    return cached.response.clone();
                }
            }
        }

        let mut guard = self.inner.write().await;
        // Re-check after acquiring the write lock in case another task refreshed.
        if let Some(cached) = guard.as_ref() {
            if cached.fetched_at.elapsed() < RATES_CACHE_TTL {
                return cached.response.clone();
            }
        }

        let response = fetch_rates().await;
        *guard = Some(CachedRates {
            fetched_at: Instant::now(),
            response: response.clone(),
        });
        response
    }
}

impl Default for RatesCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Produce a fresh rates snapshot for the supported assets.
async fn fetch_rates() -> RatesResponse {
    let timestamp = chrono::Utc::now().timestamp();
    let rates = SUPPORTED_ASSETS
        .iter()
        .map(|asset| AssetRate {
            asset: (*asset).to_string(),
            ngn: ngn_rate_for(asset),
        })
        .collect();

    RatesResponse {
        timestamp,
        source: RATES_SOURCE.to_string(),
        rates,
    }
}

/// NGN valuation for a single asset.
fn ngn_rate_for(asset: &str) -> f64 {
    match asset {
        "XLM" => 1_200.0,
        "USDC" => 1_600.0,
        "NGNT" => 1.0,
        _ => 0.0,
    }
}

/// `GET /rates` — NGN rates for each supported asset.
pub async fn get_rates(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let response = state.rates_cache.get_or_refresh().await;
    (StatusCode::OK, Json(response))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/rates", get(get_rates))
}
