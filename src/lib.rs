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
            wallet_encryption_key: std::sync::Arc::new([0u8; 32]),
            payment_provider: std::sync::Arc::new(payments::paystack::PaystackProvider::new(
                "test-paystack-key".to_string(),
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
