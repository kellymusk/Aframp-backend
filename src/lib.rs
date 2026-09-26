mod api;
mod auth;
pub mod blockchain;
mod config;
mod error;
mod middleware;
mod models;
mod pagination;
pub mod otp;
pub mod payments;
pub mod services;
mod validation;

pub use auth::cookie::{CookieConfig, SameSite};
pub use config::{AppConfig, OtpProviderKind, SecretString};

use sqlx::{postgres::PgPoolOptions, PgPool};
use tokio::sync::broadcast;

/// Events broadcast to connected admin dashboard SSE clients.
#[derive(Clone, Debug)]
pub enum AdminEvent {
    NewPayment,
    NewWithdrawal,
    NewSignup,
    WithdrawalFailed,
}

impl AdminEvent {
    /// SSE event name emitted to clients.
    pub fn name(&self) -> &'static str {
        match self {
            AdminEvent::NewPayment => "new_payment",
            AdminEvent::NewWithdrawal => "new_withdrawal",
            AdminEvent::NewSignup => "new_signup",
            AdminEvent::WithdrawalFailed => "withdrawal_failed",
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub jwt_secret: SecretString,
    pub webhook_secret: SecretString,
    pub wallet_encryption_key: std::sync::Arc<[u8; 32]>,
    pub payment_provider: std::sync::Arc<dyn payments::PaymentProvider>,
    pub otp_provider: std::sync::Arc<dyn otp::OtpProvider>,
    pub otp_hmac_secret: SecretString,
    pub cookie: CookieConfig,
    pub admin_events: broadcast::Sender<AdminEvent>,
}

impl AppState {
    /// Broadcast an admin event to all connected SSE clients.
    ///
    /// Errors (e.g. no active subscribers) are intentionally ignored so that
    /// emitting an event never fails the originating request.
    pub fn emit_admin_event(&self, event: AdminEvent) {
        let _ = self.admin_events.send(event);
    }
}

pub async fn build_state(config: &AppConfig) -> Result<AppState, Box<dyn std::error::Error>> {
    let db = PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await?;
    let wallet_encryption_key = blockchain::wallet_crypto::parse_key(config.wallet_encryption_key.as_str())?;
    let otp_provider: std::sync::Arc<dyn otp::OtpProvider> = match config.otp_provider {
        OtpProviderKind::Termii => std::sync::Arc::new(otp::termii::TermiiProvider::new(
            config
                .termii_api_key
                .as_ref()
                .expect("TERMII_API_KEY is required when OTP_PROVIDER=termii")
                .as_str()
                .to_string(),
            config
                .termii_sender_id
                .clone()
                .expect("TERMII_SENDER_ID is required when OTP_PROVIDER=termii"),
        )),
        OtpProviderKind::Mock => std::sync::Arc::new(otp::mock::MockOtpProvider),
    };
    let (admin_events, _) = broadcast::channel(256);
    Ok(AppState {
        db,
        jwt_secret: config.jwt_secret.clone(),
        webhook_secret: config.webhook_secret.clone(),
        wallet_encryption_key: std::sync::Arc::new(wallet_encryption_key),
        payment_provider: std::sync::Arc::new(payments::paystack::PaystackProvider::new(
            config.paystack_secret_key.as_str().to_string(),
        )),
        otp_provider,
        otp_hmac_secret: config.otp_hmac_secret.clone(),
        cookie: config.cookie,
        admin_events,
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
        .route("/verify-otp", axum::routing::post(api::auth::verify_otp))
        .route("/logout", axum::routing::post(api::auth::logout))
        .route("/webhooks/termii", axum::routing::post(api::webhooks::termii))
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
        .route("/admin", axum::routing::get(api::admin::dashboard))
        .route("/admin/overview", axum::routing::get(api::admin::overview))
        .route("/admin/merchants", axum::routing::get(api::admin::merchants))
        .route("/admin/users", axum::routing::get(api::admin::users))
        .route("/admin/wallets", axum::routing::get(api::admin::wallets))
        .route("/admin/transactions", axum::routing::get(api::admin::transactions))
        .route("/admin/withdrawals", axum::routing::get(api::admin::withdrawals))
        .route(
            "/admin/payment-requests",
            axum::routing::get(api::admin::payment_requests),
        )
        .route("/admin/events", axum::routing::get(api::admin::events))
        .with_state(state)
        .layer(axum::middleware::from_fn(middleware::require_json_content_type))
}
