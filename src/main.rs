use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

#[derive(Clone)]
struct AppState {
    rates_cache: Arc<Mutex<Option<CachedRates>>>,
}

#[derive(Clone)]
struct CachedRates {
    fetched_at: Instant,
    payload: RatesResponse,
}

#[derive(Clone, Serialize)]
struct AssetRate {
    asset: String,
    ngn: f64,
}

#[derive(Clone, Serialize)]
struct RatesResponse {
    rates: Vec<AssetRate>,
    timestamp: u64,
    source: String,
}

const RATES_TTL: Duration = Duration::from_secs(300);
const RATES_SOURCE: &str = "aframp-internal";

fn supported_assets() -> Vec<&'static str> {
    vec!["XLM", "USDC", "NGNT"]
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn fetch_ngn_rates() -> Vec<AssetRate> {
    // Placeholder valuation source; replace with a real oracle/pricing feed.
    supported_assets()
        .into_iter()
        .map(|asset| AssetRate {
            asset: asset.to_string(),
            ngn: match asset {
                "XLM" => 250.0,
                "USDC" => 1600.0,
                "NGNT" => 1.0,
                _ => 0.0,
            },
        })
        .collect()
}

async fn get_rates(State(state): State<AppState>) -> impl IntoResponse {
    let mut cache = state.rates_cache.lock().await;

    if let Some(cached) = cache.as_ref() {
        if cached.fetched_at.elapsed() < RATES_TTL {
            return (StatusCode::OK, Json(cached.payload.clone())).into_response();
        }
    }

    let payload = RatesResponse {
        rates: fetch_ngn_rates().await,
        timestamp: now_unix(),
        source: RATES_SOURCE.to_string(),
    };

    *cache = Some(CachedRates {
        fetched_at: Instant::now(),
        payload: payload.clone(),
    });

    (StatusCode::OK, Json(payload)).into_response()
}

#[tokio::main]
async fn main() {
    let state = AppState {
        rates_cache: Arc::new(Mutex::new(None)),
    };

    let app = Router::new()
        .route("/rates", get(get_rates))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("failed to bind address");

    axum::serve(listener, app).await.expect("server error");
}
