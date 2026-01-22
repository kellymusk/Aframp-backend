use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::env;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub stellar: StellarConfig,
    pub afri: AfriConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub environment: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StellarConfig {
    pub network: String,
    pub horizon_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AfriConfig {
    pub asset_code: String,
    pub issuer_address: String,
    pub supported_currencies: Vec<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let server = ServerConfig {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env::var("PORT")
                .context("PORT not set")?
                .parse()
                .context("PORT must be a valid number")?,
            environment: env::var("ENVIRONMENT").unwrap_or_else(|_| "development".to_string()),
        };

        let database = DatabaseConfig {
            url: env::var("DATABASE_URL").context("DATABASE_URL not set")?,
            max_connections: env::var("DATABASE_MAX_CONNECTIONS")
                .unwrap_or_else(|_| "20".to_string())
                .parse()
                .context("DATABASE_MAX_CONNECTIONS must be a valid number")?,
        };

        let redis = RedisConfig {
            url: env::var("REDIS_URL").context("REDIS_URL not set")?,
        };

        let stellar = StellarConfig {
            network: env::var("STELLAR_NETWORK").context("STELLAR_NETWORK not set")?,
            horizon_url: env::var("STELLAR_HORIZON_URL").context("STELLAR_HORIZON_URL not set")?,
        };

        let supported_currencies_str =
            env::var("AFRI_SUPPORTED_CURRENCIES").context("AFRI_SUPPORTED_CURRENCIES not set")?;
        let supported_currencies: Vec<String> = supported_currencies_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let afri = AfriConfig {
            asset_code: env::var("AFRI_ASSET_CODE").context("AFRI_ASSET_CODE not set")?,
            issuer_address: env::var("AFRI_ISSUER_ADDRESS")
                .context("AFRI_ISSUER_ADDRESS not set")?,
            supported_currencies,
        };

        let config = Config {
            server,
            database,
            redis,
            stellar,
            afri,
        };

        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        // Validate port range
        if self.server.port < 1024 {
            return Err(anyhow!(
                "Port must be at least 1024, got {}",
                self.server.port
            ));
        }

        // Validate environment
        let valid_environments = ["development", "staging", "production"];
        if !valid_environments.contains(&self.server.environment.as_str()) {
            return Err(anyhow!(
                "Environment must be one of: {:?}, got {}",
                valid_environments,
                self.server.environment
            ));
        }

        // Validate URLs are not empty
        if self.database.url.trim().is_empty() {
            return Err(anyhow!("DATABASE_URL cannot be empty"));
        }

        if self.redis.url.trim().is_empty() {
            return Err(anyhow!("REDIS_URL cannot be empty"));
        }

        if self.stellar.horizon_url.trim().is_empty() {
            return Err(anyhow!("STELLAR_HORIZON_URL cannot be empty"));
        }

        // Validate Stellar network
        let valid_networks = ["testnet", "mainnet"];
        if !valid_networks.contains(&self.stellar.network.as_str()) {
            return Err(anyhow!(
                "STELLAR_NETWORK must be 'testnet' or 'mainnet', got {}",
                self.stellar.network
            ));
        }

        // Validate AFRI configuration
        if self.afri.asset_code.trim().is_empty() {
            return Err(anyhow!("AFRI_ASSET_CODE cannot be empty"));
        }

        if self.afri.issuer_address.trim().is_empty() {
            return Err(anyhow!("AFRI_ISSUER_ADDRESS cannot be empty"));
        }

        if self.afri.supported_currencies.is_empty() {
            return Err(anyhow!(
                "AFRI_SUPPORTED_CURRENCIES must contain at least one currency"
            ));
        }

        // Validate database max connections
        if self.database.max_connections == 0 {
            return Err(anyhow!("DATABASE_MAX_CONNECTIONS must be greater than 0"));
        }

        Ok(())
    }
}
