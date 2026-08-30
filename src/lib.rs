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
use std::num::NonZeroU32;
use tower_governor::{governor, Quota, RateLimiter};
use axum::middleware as axum_middleware;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub jwt_secret: std::sync::Arc<String>,
    pub webhook_secret: std::sync::Arc<String>,
    pub wallet_encryption_key: std::sync::Arc<[u8; 32]>,
    pub payment_provider: std::sync::Arc<dyn payments::PaymentProvider>,
    pub cookie: CookieConfig,
}

pub async fn build_state(config: &AppConfig) -> Result<AppState, Box<dyn std::error::Error>> {
    let db = PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await?;
    let wallet_encryption_key = blockchain::wallet_crypto::parse_key(&config.wallet_encryption_key)?;
    Ok(AppState {
        db,
        jwt_secret: config.jwt_secret.clone(),
        webhook_secret: config.webhook_secret.clone(),
        wallet_encryption_key: std::sync::Arc::new(wallet_encryption_key),
        payment_provider: std::sync::Arc::new(payments::paystack::PaystackProvider::new(
            (*config.paystack_secret_key).clone(),
        )),
        cookie: config.cookie,
    })
}

pub fn router(state: AppState) -> axum::Router {
    // Issue #949: Rate limiting for login endpoint (5 requests per minute per IP)
    let login_limiter = RateLimiter::direct(Quota::per_minute(
        NonZeroU32::new(5).unwrap(),
    ));

    axum::Router::new()
        .route("/", axum::routing::get(|| async { "aframp" }))
        .route(
            "/health",
            axum::routing::get(|| async { axum::http::StatusCode::NO_CONTENT }),
        )
        .route("/signup", axum::routing::post(api::auth::signup))
        .route(
            "/login",
            axum::routing::post(api::auth::login)
                .layer(axum_middleware::from_fn(move |req, next| {
                    rate_limit_middleware(req, next, login_limiter.clone())
                })),
        )
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

async fn rate_limit_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
    limiter: RateLimiter,
) -> Result<axum::response::Response, (axum::http::StatusCode, axum::Json<error::ApiError>)> {
    if limiter.check().is_err() {
        return Err((
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            axum::Json(error::ApiError {
                error: "too many login attempts, please try again later".to_string(),
            }),
        ));
    }
    Ok(next.run(req).await)
}
