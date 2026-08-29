use std::sync::Arc;

use crate::auth::cookie::{CookieConfig, SameSite};

#[derive(Clone)]
pub struct AppConfig {
    pub database_url: String,
    pub bind_addr: String,
    pub jwt_secret: Arc<String>,
    pub webhook_secret: Arc<String>,
    pub stellar_system_wallet: Arc<String>,
    pub stellar_horizon_url: String,
    pub stellar_poll_interval_secs: u64,
    pub wallet_encryption_key: Arc<String>,
    pub paystack_secret_key: Arc<String>,
    /// Browser origins allowed to call this API. The merchant frontend is a
    /// separate origin, so without this every request fails CORS preflight.
    pub cors_allowed_origins: Vec<String>,
    /// How the session cookie is stamped. Defaults are the deployed ones:
    /// `Secure` on, `SameSite=Lax`. Browsers treat localhost as a secure
    /// context, so the defaults also work for local development over HTTP.
    pub cookie: CookieConfig,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, String> {
        let cookie_secure = flag("COOKIE_SECURE", true)?;
        let cookie_same_site = match std::env::var("COOKIE_SAME_SITE")
            .unwrap_or_else(|_| "lax".into())
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "lax" => SameSite::Lax,
            "none" => SameSite::None,
            other => return Err(format!("COOKIE_SAME_SITE must be `lax` or `none`, got `{other}`")),
        };
        if cookie_same_site == SameSite::None && !cookie_secure {
            return Err("COOKIE_SAME_SITE=none requires COOKIE_SECURE=true; browsers reject a SameSite=None cookie that is not Secure".into());
        }

        let app_env = std::env::var("APP_ENV").unwrap_or_else(|_| "development".into());
        if app_env.trim().eq_ignore_ascii_case("production") && !cookie_secure {
            // This server speaks plain HTTP; it relies on a TLS-terminating reverse
            // proxy (Caddy/nginx) in front of it. COOKIE_SECURE=false in production
            // means the session cookie would be sent unencrypted, so refuse to start.
            tracing::error!(
                "REFUSING TO START: APP_ENV=production but COOKIE_SECURE=false. \
                 The session cookie would be sent over plain HTTP. Run this service \
                 behind a TLS-terminating reverse proxy (Caddy or nginx) and set \
                 COOKIE_SECURE=true (the default) once TLS is in place. See README.md."
            );
            return Err(
                "APP_ENV=production requires COOKIE_SECURE=true (see README.md for the reverse-proxy/TLS setup)".into(),
            );
        }

        Ok(Self {
            database_url: env("DATABASE_URL")?,
            bind_addr: std::env::var("APP_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into()),
            jwt_secret: Arc::new(env("JWT_SECRET")?),
            webhook_secret: Arc::new(env("WEBHOOK_SECRET")?),
            stellar_system_wallet: Arc::new(env("STELLAR_SYSTEM_WALLET_ADDRESS")?),
            stellar_horizon_url: std::env::var("STELLAR_HORIZON_URL")
                .unwrap_or_else(|_| "https://horizon-testnet.stellar.org".into()),
            stellar_poll_interval_secs: std::env::var("STELLAR_POLL_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            wallet_encryption_key: Arc::new(env("WALLET_ENCRYPTION_KEY")?),
            paystack_secret_key: Arc::new(env("PAYSTACK_SECRET_KEY")?),
            cors_allowed_origins: std::env::var("CORS_ALLOWED_ORIGINS")
                .unwrap_or_else(|_| "http://localhost:3001".into())
                .split(',')
                .map(|origin| origin.trim().to_string())
                .filter(|origin| !origin.is_empty())
                .collect(),
            cookie: CookieConfig {
                secure: cookie_secure,
                same_site: cookie_same_site,
            },
        })
    }
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is required"))
}

fn flag(name: &str, default: bool) -> Result<bool, String> {
    match std::env::var(name) {
        Err(_) => Ok(default),
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Ok(true),
            "0" | "false" | "no" => Ok(false),
            other => Err(format!("{name} must be true or false, got `{other}`")),
        },
    }
}
