use std::fmt;
use std::sync::Arc;

use crate::auth::cookie::{CookieConfig, SameSite};

#[derive(Clone)]
pub struct SecretString(Arc<String>);

impl SecretString {
    pub fn new(s: String) -> Self {
        SecretString(Arc::new(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl std::ops::Deref for SecretString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for SecretString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub database_url: String,
    pub bind_addr: String,
    pub jwt_secret: SecretString,
    pub webhook_secret: SecretString,
    pub stellar_system_wallet: Arc<String>,
    pub stellar_horizon_url: String,
    pub stellar_poll_interval_secs: u64,
    pub wallet_encryption_key: SecretString,
    pub paystack_secret_key: SecretString,
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

        Ok(Self {
            database_url: env("DATABASE_URL")?,
            bind_addr: std::env::var("APP_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into()),
            jwt_secret: SecretString::new(env("JWT_SECRET")?),
            webhook_secret: SecretString::new(env("WEBHOOK_SECRET")?),
            stellar_system_wallet: Arc::new(env("STELLAR_SYSTEM_WALLET_ADDRESS")?),
            stellar_horizon_url: std::env::var("STELLAR_HORIZON_URL")
                .unwrap_or_else(|_| "https://horizon-testnet.stellar.org".into()),
            stellar_poll_interval_secs: std::env::var("STELLAR_POLL_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            wallet_encryption_key: SecretString::new(env("WALLET_ENCRYPTION_KEY")?),
            paystack_secret_key: SecretString::new(env("PAYSTACK_SECRET_KEY")?),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_string_debug_redacts_secret() {
        let secret = SecretString::new("my-secret-key".to_string());
        let debug_str = format!("{:?}", secret);
        assert_eq!(debug_str, "[REDACTED]");
        assert!(!debug_str.contains("my-secret-key"));
    }

    #[test]
    fn secret_string_display_redacts_secret() {
        let secret = SecretString::new("my-secret-key".to_string());
        let display_str = format!("{}", secret);
        assert_eq!(display_str, "[REDACTED]");
        assert!(!display_str.contains("my-secret-key"));
    }

    #[test]
    fn app_config_debug_redacts_secrets() {
        let config_debug = format!(
            "{:?}",
            AppConfig {
                database_url: "postgres://localhost".to_string(),
                bind_addr: "127.0.0.1:3000".to_string(),
                jwt_secret: SecretString::new("jwt-secret-value".to_string()),
                webhook_secret: SecretString::new("webhook-secret-value".to_string()),
                stellar_system_wallet: Arc::new("GXXXXXXX".to_string()),
                stellar_horizon_url: "https://horizon.stellar.org".to_string(),
                stellar_poll_interval_secs: 60,
                wallet_encryption_key: SecretString::new("encryption-key".to_string()),
                paystack_secret_key: SecretString::new("paystack-key".to_string()),
                cors_allowed_origins: vec!["http://localhost:3001".to_string()],
                cookie: CookieConfig {
                    secure: true,
                    same_site: SameSite::Lax,
                },
            }
        );
        assert!(!config_debug.contains("jwt-secret-value"));
        assert!(!config_debug.contains("webhook-secret-value"));
        assert!(!config_debug.contains("encryption-key"));
        assert!(!config_debug.contains("paystack-key"));
    }
}
