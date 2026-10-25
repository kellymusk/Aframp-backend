mod api;
mod auth;
pub mod blockchain;
mod config;
mod error;
mod middleware;
pub mod models;
mod pagination;
pub mod otp;
pub mod payments;
pub mod services;
mod validation;

pub use auth::cookie::{CookieConfig, SameSite};
pub use config::{AppConfig, OtpProviderKind, SecretBytes, SecretString};
pub use config::{AppConfig, OtpProviderKind, SecretString, DEFAULT_MAX_REQUEST_BODY_BYTES};

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
    /// AES-256-GCM key used to encrypt wallet private keys at rest.
    ///
    /// Currently only consumed by `services::wallets::create_wallet` for
    /// *encryption*.  Decryption (needed to sign Stellar transactions) is not
    /// yet wired into any handler or the deposit worker — the worker only reads
    /// the public wallet address.
    ///
    /// TODO: wire `decrypt_wallet_secret` into the settlement/sweep feature
    /// so the platform wallet can sign outbound transactions on behalf of a
    /// merchant.  See PRD §settlement-sweep.
    pub wallet_encryption_key: std::sync::Arc<SecretBytes<32>>,
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

/// Minimum allowed value for `STELLAR_POLL_INTERVAL_SECS`.
const MIN_STELLAR_POLL_INTERVAL_SECS: u64 = 10;

/// Parse an optional numeric env var, failing fast on invalid values.
///
/// Returns `Ok(None)` when the variable is unset, `Ok(Some(v))` when it parses,
/// and a descriptive `Err` when it is set but unparseable or below `min`.
fn parse_numeric_env<T>(name: &str, min: Option<T>) -> Result<Option<T>, String>
where
    T: std::str::FromStr + PartialOrd + std::fmt::Display + Copy,
{
    match std::env::var(name) {
        Ok(raw) => {
            let value = raw
                .trim()
                .parse::<T>()
                .map_err(|_| format!("invalid value for {name}: {raw:?} is not a valid number"))?;
            if let Some(min) = min {
                if value < min {
                    return Err(format!(
                        "invalid value for {name}: {value} is below the minimum of {min}"
                    ));
                }
            }
            Ok(Some(value))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(format!("invalid value for {name}: not valid unicode"))
        }
    }
}

/// Resolve the Stellar poll interval, failing fast on invalid configuration.
pub fn stellar_poll_interval_secs() -> Result<u64, String> {
    Ok(parse_numeric_env("STELLAR_POLL_INTERVAL_SECS", Some(MIN_STELLAR_POLL_INTERVAL_SECS))?
        .unwrap_or(60))
}

/// Resolve the database max connections, failing fast on invalid configuration.
pub fn db_max_connections() -> Result<u32, String> {
    Ok(parse_numeric_env("DB_MAX_CONNECTIONS", Some(1u32))?.unwrap_or(5))
}

pub async fn build_state(config: &AppConfig) -> Result<AppState, Box<dyn std::error::Error>> {
    let max_connections = db_max_connections()?;
    let db = PgPoolOptions::new()
        .max_connections(max_connections)
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
        wallet_encryption_key: std::sync::Arc::new(SecretBytes::new(wallet_encryption_key)),
        payment_provider: std::sync::Arc::new(payments::paystack::PaystackProvider::new(
            config.paystack_secret_key.clone(),
        )),
        otp_provider,
        otp_hmac_secret: config.otp_hmac_secret.clone(),
        cookie: config.cookie,
        admin_events,
#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

use sqlx::{postgres::PgPoolOptions, PgPool};

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub jwt_secret: SecretString,
    pub webhook_secret: SecretString,
    pub wallet_encryption_key: std::sync::Arc<SecretBytes<32>>,
    pub payment_provider: std::sync::Arc<dyn payments::PaymentProvider>,
    pub otp_provider: std::sync::Arc<dyn otp::OtpProvider>,
    pub otp_hmac_secret: SecretString,
    pub cookie: CookieConfig,
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
        .route("/auth/change-password", axum::routing::post(api::auth::change_password))
        .route("/verify-email", axum::routing::get(api::auth::verify_email))
        .route(
            "/verify-email/resend",
            axum::routing::post(api::auth::resend_email_verification),
        )
        .route("/webhooks/termii", axum::routing::post(api::webhooks::termii))
        .route("/me", axum::routing::get(api::me::get))
        .route("/users/me", axum::routing::patch(api::me::update))
        .route("/merchants/me", axum::routing::patch(api::merchants::update))
        .route(
            "/merchants/me/email/confirm",
            axum::routing::post(api::merchants::confirm_email),
        )
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
            "/payment-requests/{id}/status",
            axum::routing::get(api::payment_requests::status),
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
        .route(
            "/admin/merchants/{id}/suspend",
            axum::routing::post(api::admin::suspend_merchant),
        )
        .route(
            "/admin/merchants/{id}/unsuspend",
            axum::routing::post(api::admin::unsuspend_merchant),
        )
        .route(
            "/admin/users/{id}/unlock",
            axum::routing::post(api::admin::unlock_user),
        )
        .with_state(state)
        .layer(axum::middleware::from_fn(middleware::require_json_content_type))
pub fn app() -> Router {
    Router::new().route("/health", get(health))
}

pub async fn build_state(config: &AppConfig) -> Result<AppState, Box<dyn std::error::Error>> {
    let db = PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .min_connections(config.db_min_connections)
        .acquire_timeout(std::time::Duration::from_secs(config.db_acquire_timeout_secs))
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
    Ok(AppState {
        db,
        jwt_secret: config.jwt_secret.clone(),
        webhook_secret: config.webhook_secret.clone(),
        wallet_encryption_key: std::sync::Arc::new(SecretBytes::new(wallet_encryption_key)),
        payment_provider: std::sync::Arc::new(payments::paystack::PaystackProvider::new(
            config.paystack_secret_key.clone(),
        )),
        otp_provider,
        otp_hmac_secret: config.otp_hmac_secret.clone(),
        cookie: config.cookie,
    })
}

pub fn router(state: AppState) -> axum::Router {
    axum::Router::new()
        .route("/", axum::routing::get(|| async { "aframp" }))
        .route("/health", axum::routing::get(api::health::health))
        .route("/signup", axum::routing::post(api::auth::signup))
        .route("/login", axum::routing::post(api::auth::login))
        .route("/verify-otp", axum::routing::post(api::auth::verify_otp))
        .route("/logout", axum::routing::post(api::auth::logout))
        .route("/auth/change-password", axum::routing::post(api::auth::change_password))
        .route("/verify-email", axum::routing::get(api::auth::verify_email))
        .route(
            "/verify-email/resend",
            axum::routing::post(api::auth::resend_email_verification),
        )
        .route("/auth/refresh", axum::routing::post(api::auth::refresh))
        .route("/webhooks/termii", axum::routing::post(api::webhooks::termii))
        .route(
            "/me",
            axum::routing::get(api::me::get)
                .patch(api::me::update)
                .delete(api::me::delete),
        )
        .route("/users/me", axum::routing::patch(api::me::update))
        .route("/merchants/me", axum::routing::patch(api::merchants::update))
        .route(
            "/merchants/me/email/confirm",
            axum::routing::post(api::merchants::confirm_email),
        )
        .route("/merchants/me", axum::routing::get(api::merchants::get))
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
            axum::routing::get(api::payment_requests::get)
                .delete(api::payment_requests::cancel),
        )
        .route(
            "/payment-requests/{id}/cancel",
            axum::routing::post(api::payment_requests::cancel),
        )
        .route(
            "/payment-requests/{id}/status",
            axum::routing::get(api::payment_requests::status),
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
        .with_state(state)
        .layer(axum::middleware::from_fn(middleware::require_json_content_type))
}

#[cfg(test)]
mod cors_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let db = PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@localhost:5432/aframp")
            .expect("lazy pool");
        AppState {
            db,
            jwt_secret: SecretString::new("test-jwt-secret".to_string()),
            webhook_secret: SecretString::new("test-webhook-secret".to_string()),
            wallet_encryption_key: std::sync::Arc::new(SecretBytes::new([0u8; 32])),
            payment_provider: std::sync::Arc::new(payments::paystack::PaystackProvider::new(
                SecretString::new("test-paystack-key".to_string()),
            )),
            otp_provider: std::sync::Arc::new(otp::mock::MockOtpProvider),
            otp_hmac_secret: SecretString::new("test-otp-hmac-secret".to_string()),
            cookie: CookieConfig::default(),
        }
    }

    async fn preflight(method: Method) -> StatusCode {
        let app = router(test_state());
        let request = Request::builder()
            .method(Method::OPTIONS)
            .uri("/me")
            .header(header::ORIGIN, "https://app.aframp.com")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, method.as_str())
            .body(Body::empty())
            .expect("request");
        app.oneshot(request).await.expect("response").status()
    }

    #[tokio::test]
    async fn cors_preflight_allows_patch() {
        let status = preflight(Method::PATCH).await;
        assert!(
            status.is_success(),
            "PATCH preflight should be allowed, got {status}"
        );
    }

    #[tokio::test]
    async fn cors_preflight_allows_delete() {
        let status = preflight(Method::DELETE).await;
        assert!(
            status.is_success(),
            "DELETE preflight should be allowed, got {status}"
        );
    }
}

#[cfg(test)]
mod config_env_tests {
    use super::*;

    #[test]
    fn invalid_stellar_poll_interval_returns_err() {
        std::env::set_var("STELLAR_POLL_INTERVAL_SECS", "abc");
        let result = stellar_poll_interval_secs();
        std::env::remove_var("STELLAR_POLL_INTERVAL_SECS");
        assert!(result.is_err(), "expected Err for unparseable poll interval");
        assert!(result.unwrap_err().contains("STELLAR_POLL_INTERVAL_SECS"));
    }

    #[test]
    fn too_low_stellar_poll_interval_returns_err() {
        std::env::set_var("STELLAR_POLL_INTERVAL_SECS", "1");
        let result = stellar_poll_interval_secs();
        std::env::remove_var("STELLAR_POLL_INTERVAL_SECS");
        assert!(result.is_err(), "expected Err for poll interval below minimum");
    }

    #[test]
    fn invalid_db_max_connections_returns_err() {
        std::env::set_var("DB_MAX_CONNECTIONS", "not-a-number");
        let result = db_max_connections();
        std::env::remove_var("DB_MAX_CONNECTIONS");
        assert!(result.is_err(), "expected Err for unparseable DB_MAX_CONNECTIONS");
    }
}
