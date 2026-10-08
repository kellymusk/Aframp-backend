use std::fmt;
use std::sync::Arc;

use crate::auth::cookie::{CookieConfig, SameSite};

/// Default request body limit (1MB) used when `MAX_REQUEST_BODY_BYTES` is unset.
pub const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;

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

/// Which SMS backend delivers OTP codes. `Mock` logs the code instead of
/// sending it (see `otp::mock`), so local dev never needs live Termii
/// credentials — the walkthrough in the OTP plan runs entirely on `Mock`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtpProviderKind {
    Mock,
    Termii,
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
    /// Maximum number of Horizon requests the deposit worker keeps in flight
    /// at once. Bounds the fan-out when polling many wallet addresses so a
    /// large merchant set doesn't open thousands of sockets simultaneously.
    pub stellar_poll_concurrency: usize,
    pub wallet_encryption_key: SecretString,
    pub paystack_secret_key: SecretString,
    /// Keys the HMAC that OTP codes are stored under. A bare hash of a
    /// 6-digit code is trivially reversible by anyone with DB read access
    /// (only ~1M possible values) — this secret is what makes the digest
    /// unrecoverable without it. Never reused for anything else.
    pub otp_hmac_secret: SecretString,
    pub otp_provider: OtpProviderKind,
    /// Required when `otp_provider` is `Termii`; absent when it's `Mock`.
    pub termii_api_key: Option<SecretString>,
    pub termii_sender_id: Option<String>,
    /// Browser origins allowed to call this API. The merchant frontend is a
    /// separate origin, so without this every request fails CORS preflight.
    pub cors_allowed_origins: Vec<String>,
    /// How the session cookie is stamped. Defaults are the deployed ones:
    /// `Secure` on, `SameSite=Lax`. Browsers treat localhost as a secure
    /// context, so the defaults also work for local development over HTTP.
    pub cookie: CookieConfig,
    /// Maximum accepted request body size in bytes. Applied via
    /// `RequestBodyLimitLayer`; requests over this limit are rejected with a
    /// 413 before reaching a handler. Defaults to 1MB.
    pub max_request_body_bytes: usize,
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

        let otp_provider = match std::env::var("OTP_PROVIDER")
            .unwrap_or_else(|_| "termii".into())
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "termii" => OtpProviderKind::Termii,
            "mock" => OtpProviderKind::Mock,
            other => return Err(format!("OTP_PROVIDER must be `termii` or `mock`, got `{other}`")),
        };
        let (termii_api_key, termii_sender_id) = if otp_provider == OtpProviderKind::Termii {
            (
                Some(SecretString::new(env("TERMII_API_KEY")?)),
                Some(env("TERMII_SENDER_ID")?),
            )
        } else {
            (None, None)
        };

        let stellar_poll_concurrency = std::env::var("STELLAR_POLL_CONCURRENCY")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(50);
        let max_request_body_bytes = match std::env::var("MAX_REQUEST_BODY_BYTES") {
            Err(_) => DEFAULT_MAX_REQUEST_BODY_BYTES,
            Ok(value) => value
                .trim()
                .parse::<usize>()
                .map_err(|_| format!("MAX_REQUEST_BODY_BYTES must be a positive integer, got `{value}`"))?,
        };
        if max_request_body_bytes == 0 {
            return Err("MAX_REQUEST_BODY_BYTES must be greater than 0".into());
        }

        Ok(Self {
            database_url: env("DATABASE_URL")?,
            bind_addr: std::env::var("APP_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into()),
            jwt_secret: secret("JWT_SECRET")?,
            webhook_secret: secret("WEBHOOK_SECRET")?,
            jwt_secret: SecretString::new(secret_min32("JWT_SECRET")?),
            webhook_secret: SecretString::new(secret_min32("WEBHOOK_SECRET")?),
            stellar_system_wallet: Arc::new(env("STELLAR_SYSTEM_WALLET_ADDRESS")?),
            stellar_horizon_url: std::env::var("STELLAR_HORIZON_URL")
                .unwrap_or_else(|_| "https://horizon-testnet.stellar.org".into()),
            stellar_poll_interval_secs: std::env::var("STELLAR_POLL_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            stellar_poll_concurrency,
            wallet_encryption_key: SecretString::new(env("WALLET_ENCRYPTION_KEY")?),
            paystack_secret_key: SecretString::new(env("PAYSTACK_SECRET_KEY")?),
            otp_hmac_secret: secret("OTP_HMAC_SECRET")?,
            otp_provider,
            termii_api_key,
            termii_sender_id,
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
            max_request_body_bytes,
        })
    }
}

/// Minimum number of characters required for HMAC/signing secrets.
/// HMAC-SHA256 security degrades significantly with keys shorter than
/// 32 bytes; this constant makes the threshold explicit and testable.
pub const MIN_SECRET_LEN: usize = 32;

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is required"))
}

/// Read a required secret from the environment and enforce a minimum length.
///
/// A secret that is too short is rejected at startup with a clear error
/// message and a `openssl rand -hex 32` generation hint so the operator
/// knows exactly what to do.
fn secret(name: &str) -> Result<SecretString, String> {
    let value = env(name)?;
    if value.len() < MIN_SECRET_LEN {
        return Err(format!(
            "{name} is too short ({} chars); it must be at least {MIN_SECRET_LEN} characters \
             to provide adequate security. Generate a strong value with: \
             openssl rand -hex 32",
            value.len()
        ));
    }
    Ok(SecretString::new(value))
/// Reads an environment variable and rejects it if it is shorter than 32
/// characters. HMAC-SHA256 is only as strong as its key; keys below 32 bytes
/// fall below the NIST SP 800-107 minimum recommendation.
///
/// Generate a safe value with: `openssl rand -hex 32`
fn secret_min32(name: &str) -> Result<String, String> {
    let value = env(name)?;
    if value.len() < 32 {
        return Err(format!(
            "{name} must be at least 32 characters (got {}). \
             Generate a strong secret with: openssl rand -hex 32",
            value.len()
        ));
    }
    Ok(value)
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

    /// Populates every required env var with valid values so `AppConfig::from_env`
    /// can succeed. Call this before overriding individual vars in a test.
    fn set_valid_env() {
        // A 64-char hex string — well above the 32-char minimum.
        let long_secret = "a".repeat(64);
        std::env::set_var("DATABASE_URL", "postgres://localhost/test");
        std::env::set_var("WALLET_ENCRYPTION_KEY", &long_secret);
        std::env::set_var("STELLAR_SYSTEM_WALLET_ADDRESS", "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN");
        std::env::set_var("PAYSTACK_SECRET_KEY", "sk_test_placeholder");
        std::env::set_var("OTP_HMAC_SECRET", &long_secret);
        std::env::set_var("OTP_PROVIDER", "mock");
        // Set the secrets to valid values; individual tests may override these.
        std::env::set_var("JWT_SECRET", &long_secret);
        std::env::set_var("WEBHOOK_SECRET", &long_secret);
    }

    #[test]
    fn jwt_secret_too_short_is_rejected() {
        set_valid_env();
        std::env::set_var("JWT_SECRET", "tooshort");

        let err = AppConfig::from_env().unwrap_err();
        assert!(
            err.contains("JWT_SECRET"),
            "error should name the failing variable: {err}"
        );
        assert!(
            err.contains("32"),
            "error should mention the 32-character minimum: {err}"
        );
        assert!(
            err.contains("openssl rand -hex 32"),
            "error should include the generation hint: {err}"
        );
    }

    #[test]
    fn webhook_secret_too_short_is_rejected() {
        set_valid_env();
        std::env::set_var("WEBHOOK_SECRET", "tooshort");

        let err = AppConfig::from_env().unwrap_err();
        assert!(
            err.contains("WEBHOOK_SECRET"),
            "error should name the failing variable: {err}"
        );
        assert!(
            err.contains("32"),
            "error should mention the 32-character minimum: {err}"
        );
        assert!(
            err.contains("openssl rand -hex 32"),
            "error should include the generation hint: {err}"
        );
    }

    #[test]
    fn secret_exactly_32_chars_is_accepted() {
        set_valid_env();
        // Exactly 32 characters — right at the boundary, must pass.
        std::env::set_var("JWT_SECRET", "a".repeat(32));
        std::env::set_var("WEBHOOK_SECRET", "b".repeat(32));

        let result = AppConfig::from_env();
        assert!(
            result.is_ok(),
            "32-character secrets should be accepted: {:?}",
            result.err()
        );
    }

    #[test]
    fn jwt_secret_31_chars_is_rejected() {
        set_valid_env();
        // 31 characters — one below the boundary, must fail.
        std::env::set_var("JWT_SECRET", "a".repeat(31));

        let err = AppConfig::from_env().unwrap_err();
        assert!(err.contains("JWT_SECRET"), "error must identify the variable: {err}");
        assert!(err.contains("31"), "error should report the actual length: {err}");
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
                stellar_poll_concurrency: 50,
                wallet_encryption_key: SecretString::new("encryption-key".to_string()),
                paystack_secret_key: SecretString::new("paystack-key".to_string()),
                otp_hmac_secret: SecretString::new("otp-hmac-secret-value".to_string()),
                otp_provider: OtpProviderKind::Termii,
                termii_api_key: Some(SecretString::new("termii-key".to_string())),
                termii_sender_id: Some("Aframp".to_string()),
                cors_allowed_origins: vec!["http://localhost:3001".to_string()],
                cookie: CookieConfig {
                    secure: true,
                    same_site: SameSite::Lax,
                },
                max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
            }
        );
        assert!(!config_debug.contains("jwt-secret-value"));
        assert!(!config_debug.contains("webhook-secret-value"));
        assert!(!config_debug.contains("encryption-key"));
        assert!(!config_debug.contains("paystack-key"));
        assert!(!config_debug.contains("otp-hmac-secret-value"));
        assert!(!config_debug.contains("termii-key"));
    }

    #[test]
    fn stellar_poll_concurrency_defaults_to_50() {
        // The env var is unset in the test process, so the default applies.
        let concurrency = std::env::var("STELLAR_POLL_CONCURRENCY")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(50);
        assert_eq!(concurrency, 50);
    }

    #[test]
    fn max_request_body_bytes_defaults_to_one_megabyte() {
        assert_eq!(DEFAULT_MAX_REQUEST_BODY_BYTES, 1024 * 1024);
    }

    // --- Secret minimum-length enforcement (issue #1099) ---

    /// A 32-character secret is exactly at the threshold and must be accepted.
    #[test]
    fn secret_fn_accepts_exactly_32_chars() {
        let name = "TEST_SECRET_EXACTLY_32";
        // exactly 32 ASCII characters
        let value = "a".repeat(MIN_SECRET_LEN);
        std::env::set_var(name, &value);
        let result = super::secret(name);
        std::env::remove_var(name);
        assert!(result.is_ok(), "expected Ok for a {MIN_SECRET_LEN}-char secret");
    }

    /// A 64-character secret (typical `openssl rand -hex 32` output) must be accepted.
    #[test]
    fn secret_fn_accepts_64_char_hex_secret() {
        let name = "TEST_SECRET_64";
        let value = "a".repeat(64);
        std::env::set_var(name, &value);
        let result = super::secret(name);
        std::env::remove_var(name);
        assert!(result.is_ok(), "expected Ok for a 64-char secret");
    }

    /// A secret shorter than 32 characters must be rejected with a descriptive error.
    #[test]
    fn secret_fn_rejects_short_otp_hmac_secret() {
        let name = "OTP_HMAC_SECRET_TEST_SHORT";
        std::env::set_var(name, "tooshort");
        let result = super::secret(name);
        std::env::remove_var(name);
        let err = result.expect_err("expected Err for a short secret");
        assert!(
            err.contains("too short"),
            "error message should mention 'too short', got: {err}"
        );
        assert!(
            err.contains("openssl rand -hex 32"),
            "error message should include generation hint, got: {err}"
        );
    }

    /// jwt_secret shorter than 32 chars must be rejected.
    #[test]
    fn secret_fn_rejects_short_jwt_secret() {
        let name = "JWT_SECRET_TEST_SHORT";
        std::env::set_var(name, "weak");
        let result = super::secret(name);
        std::env::remove_var(name);
        assert!(result.is_err(), "expected Err for JWT_SECRET shorter than {MIN_SECRET_LEN} chars");
    }

    /// webhook_secret shorter than 32 chars must be rejected.
    #[test]
    fn secret_fn_rejects_short_webhook_secret() {
        let name = "WEBHOOK_SECRET_TEST_SHORT";
        std::env::set_var(name, "test");
        let result = super::secret(name);
        std::env::remove_var(name);
        assert!(result.is_err(), "expected Err for WEBHOOK_SECRET shorter than {MIN_SECRET_LEN} chars");
    }

    /// A missing secret must still return a "required" error, not a length error.
    #[test]
    fn secret_fn_returns_required_error_when_missing() {
        let name = "DEFINITELY_UNSET_SECRET_XYZ";
        std::env::remove_var(name);
        let result = super::secret(name);
        let err = result.expect_err("expected Err for missing secret");
        assert!(
            err.contains("required"),
            "error message should say 'required' for a missing var, got: {err}"
        );
    }

    /// Boundary: a 31-character value is one short of the minimum and must be rejected.
    #[test]
    fn secret_fn_rejects_31_char_secret() {
        let name = "TEST_SECRET_31_CHARS";
        let value = "b".repeat(MIN_SECRET_LEN - 1);
        std::env::set_var(name, &value);
        let result = super::secret(name);
        std::env::remove_var(name);
        assert!(
            result.is_err(),
            "expected Err for a {}-char secret (one below minimum)",
            MIN_SECRET_LEN - 1
        );
    }
}
