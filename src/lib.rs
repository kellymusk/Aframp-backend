mod api;
mod auth;
pub mod blockchain;
mod config;
mod error;
mod middleware;
mod models;
pub mod payments;
pub mod services;
mod validation;

pub use auth::cookie::{CookieConfig, SameSite};
pub use config::AppConfig;

use sqlx::{postgres::PgPoolOptions, PgPool};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub jwt_secret: std::sync::Arc<String>,
    pub webhook_secret: std::sync::Arc<String>,
    pub wallet_encryption_key: std::sync::Arc<Zeroizing<[u8; 32]>>,
    pub payment_provider: std::sync::Arc<dyn payments::PaymentProvider>,
    pub cookie: CookieConfig,
}

pub async fn build_state(config: &AppConfig) -> Result<AppState, Box<dyn std::error::Error>> {
    let db = PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await?;
    Ok(AppState {
        db,
        jwt_secret: config.jwt_secret.clone(),
        webhook_secret: config.webhook_secret.clone(),
        wallet_encryption_key: config.wallet_encryption_key.clone(),
        payment_provider: std::sync::Arc::new(payments::paystack::PaystackProvider::new(
            (*config.paystack_secret_key).clone(),
        )),
        cookie: config.cookie,
    })
}

pub fn router(state: AppState) -> axum::Router {
    axum::Router::new()
        .route("/", axum::routing::get(|| async { "aframp" }))
        .route(
            "/health",
            axum::routing::get(|| async { axum::http::StatusCode::NO_CONTENT }),
        )
        .route("/signup", axum::routing::post(api::auth::signup))
        .route("/login", axum::routing::post(api::auth::login))
        .route("/logout", axum::routing::post(api::auth::logout))
        .route("/me", axum::routing::get(api::me::get))
        .route("/wallet/create", axum::routing::post(api::wallets::create))
        .route("/wallet", axum::routing::get(api::wallets::get))
        .route("/balance", axum::routing::get(api::balances::get))
        .route("/transactions", axum::routing::get(api::transactions::list))
        .route("/withdraw", axum::routing::post(api::withdrawals::create))
        .route("/withdrawals", axum::routing::get(api::withdrawals::list))
        .route(
            "/payment-requests",
            axum::routing::post(api::payment_requests::create)
                .get(api::payment_requests::list),
        )
        .route(
            "/payment-requests/{id}",
            axum::routing::get(api::payment_requests::get),
        )
        .with_state(state)
        .layer(axum::middleware::from_fn(middleware::require_json_content_type))
}
