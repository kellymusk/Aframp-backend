//! Application configuration module
//! Handles environment variable loading, configuration validation, and application settings

use std::env;

/// Main application configuration
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub cache: CacheConfig,
    pub logging: LoggingConfig,
    pub stellar: StellarConfig,
}

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub cors_allowed_origins: Vec<String>,
}

/// Database configuration
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub connection_timeout: u64,   // seconds
    pub idle_timeout: Option<u64>, // seconds
}

/// Cache configuration
#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub redis_url: String,
    pub default_ttl: u64, // seconds
    pub max_connections: u32,
}

/// Logging configuration
#[derive(Debug, Clone)]
pub struct LoggingConfig {
    pub level: String,
    pub format: LogFormat,
    pub enable_tracing: bool,
}

/// Log format options
#[derive(Debug, Clone)]
pub enum LogFormat {
    Json,
    Plain,
}

/// Stellar-specific configuration
#[derive(Debug, Clone)]
pub struct StellarConfig {
    pub network: String,
    pub horizon_url: String,
    pub request_timeout: u64, // seconds
    pub max_retries: u32,
    pub health_check_interval: u64, // seconds
}

impl AppConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self, ConfigError> {
        // Load .env file if it exists
        let _ = dotenv::dotenv().ok();

        Ok(AppConfig {
            server: ServerConfig::from_env()?,
            database: DatabaseConfig::from_env()?,
            cache: CacheConfig::from_env()?,
            logging: LoggingConfig::from_env()?,
            stellar: StellarConfig::from_env()?,
        })
    }

    /// Validate the entire configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.server.validate()?;
        self.database.validate()?;
        self.cache.validate()?;
        self.stellar.validate()?;

        Ok(())
    }
}

impl ServerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(ServerConfig {
            host: env::var("SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            port: env::var("SERVER_PORT")
                .unwrap_or_else(|_| "8000".to_string())
                .parse()
                .map_err(|_| ConfigError::InvalidValue("SERVER_PORT".to_string()))?,
            cors_allowed_origins: env::var("CORS_ALLOWED_ORIGINS")
                .unwrap_or_else(|_| "http://localhost,http://127.0.0.1".to_string())
                .split(',')
                .map(|s| s.trim().to_string())
                .collect(),
        })
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.port == 0 {
            return Err(ConfigError::InvalidValue(
                "SERVER_PORT cannot be 0".to_string(),
            ));
        }

        if self.host.is_empty() {
            return Err(ConfigError::InvalidValue(
                "SERVER_HOST cannot be empty".to_string(),
            ));
        }

        Ok(())
    }
}

impl DatabaseConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(DatabaseConfig {
            url: env::var("DATABASE_URL")
                .map_err(|_| ConfigError::MissingVariable("DATABASE_URL".to_string()))?,
            max_connections: env::var("DB_MAX_CONNECTIONS")
                .unwrap_or_else(|_| "20".to_string())
                .parse()
                .map_err(|_| ConfigError::InvalidValue("DB_MAX_CONNECTIONS".to_string()))?,
            min_connections: env::var("DB_MIN_CONNECTIONS")
                .unwrap_or_else(|_| "5".to_string())
                .parse()
                .map_err(|_| ConfigError::InvalidValue("DB_MIN_CONNECTIONS".to_string()))?,
            connection_timeout: env::var("DB_CONNECTION_TIMEOUT")
                .unwrap_or_else(|_| "30".to_string())
                .parse()
                .map_err(|_| ConfigError::InvalidValue("DB_CONNECTION_TIMEOUT".to_string()))?,
            idle_timeout: env::var("DB_IDLE_TIMEOUT")
                .ok()
                .and_then(|val| val.parse().ok()),
        })
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.url.is_empty() {
            return Err(ConfigError::InvalidValue("DATABASE_URL".to_string()));
        }

        if self.max_connections == 0 {
            return Err(ConfigError::InvalidValue("DB_MAX_CONNECTIONS".to_string()));
        }

        if self.min_connections > self.max_connections {
            return Err(ConfigError::InvalidValue(
                "DB_MIN_CONNECTIONS must be <= DB_MAX_CONNECTIONS".to_string(),
            ));
        }

        Ok(())
    }
}

impl CacheConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(CacheConfig {
            redis_url: env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string()),
            default_ttl: env::var("CACHE_DEFAULT_TTL")
                .unwrap_or_else(|_| "3600".to_string())
                .parse()
                .map_err(|_| ConfigError::InvalidValue("CACHE_DEFAULT_TTL".to_string()))?,
            max_connections: env::var("CACHE_MAX_CONNECTIONS")
                .unwrap_or_else(|_| "10".to_string())
                .parse()
                .map_err(|_| ConfigError::InvalidValue("CACHE_MAX_CONNECTIONS".to_string()))?,
        })
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.redis_url.is_empty() {
            return Err(ConfigError::InvalidValue("REDIS_URL".to_string()));
        }

        // Basic validation of Redis URL format
        if !self.redis_url.starts_with("redis://") && !self.redis_url.starts_with("rediss://") {
            return Err(ConfigError::InvalidValue(
                "REDIS_URL must start with redis:// or rediss://".to_string(),
            ));
        }

        Ok(())
    }
}

impl LoggingConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(LoggingConfig {
            level: env::var("LOG_LEVEL").unwrap_or_else(|_| "INFO".to_string()),
            format: match env::var("LOG_FORMAT")
                .unwrap_or_else(|_| "plain".to_string())
                .as_str()
            {
                "json" => LogFormat::Json,
                _ => LogFormat::Plain,
            },
            enable_tracing: env::var("ENABLE_TRACING")
                .unwrap_or_else(|_| "false".to_string())
                .parse()
                .map_err(|_| ConfigError::InvalidValue("ENABLE_TRACING".to_string()))?,
        })
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let valid_levels = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];
        if !valid_levels.contains(&self.level.to_uppercase().as_str()) {
            return Err(ConfigError::InvalidValue("LOG_LEVEL".to_string()));
        }

        Ok(())
    }
}

impl StellarConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(StellarConfig {
            network: env::var("STELLAR_NETWORK").unwrap_or_else(|_| "testnet".to_string()),
            horizon_url: env::var("STELLAR_HORIZON_URL").unwrap_or_else(|_| {
                if env::var("STELLAR_NETWORK").unwrap_or_else(|_| "testnet".to_string())
                    == "mainnet"
                {
                    "https://horizon.stellar.org".to_string()
                } else {
                    "https://horizon-testnet.stellar.org".to_string()
                }
            }),
            request_timeout: env::var("STELLAR_REQUEST_TIMEOUT")
                .unwrap_or_else(|_| "15".to_string())
                .parse()
                .map_err(|_| ConfigError::InvalidValue("STELLAR_REQUEST_TIMEOUT".to_string()))?,
            max_retries: env::var("STELLAR_MAX_RETRIES")
                .unwrap_or_else(|_| "3".to_string())
                .parse()
                .map_err(|_| ConfigError::InvalidValue("STELLAR_MAX_RETRIES".to_string()))?,
            health_check_interval: env::var("STELLAR_HEALTH_CHECK_INTERVAL")
                .unwrap_or_else(|_| "30".to_string())
                .parse()
                .map_err(|_| {
                    ConfigError::InvalidValue("STELLAR_HEALTH_CHECK_INTERVAL".to_string())
                })?,
        })
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let valid_networks = ["testnet", "mainnet", "futurenet"];
        if !valid_networks.contains(&self.network.as_str()) {
            return Err(ConfigError::InvalidValue("STELLAR_NETWORK".to_string()));
        }

        if self.horizon_url.is_empty() {
            return Err(ConfigError::InvalidValue("STELLAR_HORIZON_URL".to_string()));
        }

        if !self.horizon_url.starts_with("http://") && !self.horizon_url.starts_with("https://") {
            return Err(ConfigError::InvalidValue(
                "STELLAR_HORIZON_URL must be a valid URL".to_string(),
            ));
        }

        if self.request_timeout == 0 {
            return Err(ConfigError::InvalidValue(
                "STELLAR_REQUEST_TIMEOUT".to_string(),
            ));
        }

        Ok(())
    }
}

/// Configuration error types
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Missing environment variable: {0}")]
    MissingVariable(String),

    #[error("Invalid value for configuration: {0}")]
    InvalidValue(String),

    #[error("Validation failed: {0}")]
    ValidationFailed(String),
}

impl From<std::num::ParseIntError> for ConfigError {
    fn from(_: std::num::ParseIntError) -> Self {
        ConfigError::InvalidValue("Failed to parse integer value".to_string())
    }
}

impl From<std::num::ParseFloatError> for ConfigError {
    fn from(_: std::num::ParseFloatError) -> Self {
        ConfigError::InvalidValue("Failed to parse float value".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_config_validation() {
        let config = ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8000,
            cors_allowed_origins: vec!["http://localhost".to_string()],
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_invalid_port_validation() {
        let config = ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0, // Invalid port
            cors_allowed_origins: vec![],
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_empty_host_validation() {
        let config = ServerConfig {
            host: "".to_string(),
            port: 8000,
            cors_allowed_origins: vec![],
        };

        assert!(config.validate().is_err());
    }
}
