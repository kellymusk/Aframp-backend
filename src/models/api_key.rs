use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ApiKey {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub key_prefix: String,
    pub environment: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Environment a merchant-scoped API key is issued for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyEnvironment {
    Test,
    Live,
}

impl ApiKeyEnvironment {
    /// Prefix used for keys issued in this environment (`sk_test_` / `sk_live_`).
    pub fn key_prefix(&self) -> &'static str {
        match self {
            ApiKeyEnvironment::Test => "sk_test_",
            ApiKeyEnvironment::Live => "sk_live_",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ApiKeyEnvironment::Test => "test",
            ApiKeyEnvironment::Live => "live",
        }
    }
}

impl std::str::FromStr for ApiKeyEnvironment {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "test" => Ok(ApiKeyEnvironment::Test),
            "live" => Ok(ApiKeyEnvironment::Live),
            other => Err(format!("invalid api key environment: {other}")),
        }
    }
}

/// Response returned when a new API key is issued. The raw `key` is only ever
/// surfaced here; only its hash is persisted in the `api_keys` table.
#[derive(Debug, Clone, Serialize)]
pub struct ApiKeyIssued {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub key: String,
    pub key_prefix: String,
    pub environment: String,
    pub created_at: DateTime<Utc>,
}

impl ApiKeyIssued {
    pub fn new(
        id: Uuid,
        merchant_id: Uuid,
        key: String,
        environment: ApiKeyEnvironment,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            merchant_id,
            key,
            key_prefix: environment.key_prefix().to_string(),
            environment: environment.as_str().to_string(),
            created_at,
        }
    }
}
