use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DeveloperAccount {
    pub id: Uuid,
    pub email: String,
    pub full_name: String,
    pub organisation: Option<String>,
    pub country: String,
    pub use_case_description: String,
    pub status_code: String,
    pub access_tier_code: String,
    pub email_verified: bool,
    pub email_verification_token: Option<String>,
    pub email_verification_expires_at: Option<DateTime<Utc>>,
    pub identity_verification_status: String,
    pub identity_verification_data: Option<serde_json::Value>,
    pub identity_verified_at: Option<DateTime<Utc>>,
    pub suspended_at: Option<DateTime<Utc>>,
    pub suspension_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDeveloperAccountRequest {
    pub email: String,
    pub full_name: String,
    pub organisation: Option<String>,
    pub country: String,
    pub use_case_description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDeveloperAccountRequest {
    pub full_name: Option<String>,
    pub organisation: Option<String>,
    pub country: Option<String>,
    pub use_case_description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityVerificationRequest {
    pub full_legal_name: String,
    pub government_id_type: String,
    pub government_id_number: String,
    pub business_registration_number: Option<String>,
    pub additional_documents: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DeveloperApplication {
    pub id: Uuid,
    pub developer_account_id: Uuid,
    pub name: String,
    pub description: String,
    pub intended_use_case: String,
    pub status: String,
    pub sandbox_wallet_address: Option<String>,
    pub sandbox_wallet_secret: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApplicationRequest {
    pub name: String,
    pub description: String,
    pub intended_use_case: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateApplicationRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub intended_use_case: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ApiKey {
    pub id: Uuid,
    pub application_id: Uuid,
    pub key_prefix: String,
    pub key_hash: String,
    pub key_name: String,
    pub environment: String,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub usage_count: i32,
    pub rate_limit_per_minute: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApiKeyRequest {
    pub key_name: String,
    pub environment: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub rate_limit_per_minute: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct OAuthClient {
    pub id: Uuid,
    pub application_id: Uuid,
    pub client_id: String,
    pub client_secret_hash: String,
    pub client_name: String,
    pub environment: String,
    pub redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateOAuthClientRequest {
    pub client_name: String,
    pub environment: String,
    pub redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateOAuthClientRequest {
    pub client_name: Option<String>,
    pub redirect_uris: Option<Vec<String>>,
    pub scopes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WebhookConfiguration {
    pub id: Uuid,
    pub application_id: Uuid,
    pub webhook_url: String,
    pub secret_token: Option<String>,
    pub events: Vec<String>,
    pub status: String,
    pub delivery_success_rate: rust_decimal::Decimal,
    pub average_delivery_latency: i32,
    pub failed_delivery_count: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWebhookConfigurationRequest {
    pub webhook_url: String,
    pub secret_token: Option<String>,
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateWebhookConfigurationRequest {
    pub webhook_url: Option<String>,
    pub secret_token: Option<String>,
    pub events: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ProductionAccessRequest {
    pub id: Uuid,
    pub application_id: Uuid,
    pub developer_account_id: Uuid,
    pub production_use_case: String,
    pub expected_transaction_volume: String,
    pub supported_countries: Vec<String>,
    pub business_details: Option<serde_json::Value>,
    pub status: String,
    pub reviewed_by_admin_id: Option<Uuid>,
    pub review_notes: Option<String>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProductionAccessRequest {
    pub production_use_case: String,
    pub expected_transaction_volume: String,
    pub supported_countries: Vec<String>,
    pub business_details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminProductionAccessReview {
    pub status: String,
    pub review_notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct UsageStatistics {
    pub id: Uuid,
    pub application_id: Uuid,
    pub api_key_id: Option<Uuid>,
    pub endpoint: String,
    pub method: String,
    pub status_code: i32,
    pub response_time_ms: Option<i32>,
    pub request_size_bytes: Option<i32>,
    pub response_size_bytes: Option<i32>,
    pub timestamp: DateTime<Utc>,
    pub environment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WebhookDeliveryLog {
    pub id: Uuid,
    pub webhook_configuration_id: Uuid,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub delivery_url: String,
    pub status: String,
    pub http_status_code: Option<i32>,
    pub response_body: Option<String>,
    pub delivery_attempts: i32,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DeveloperAccountStatus {
    pub code: String,
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AccessTier {
    pub code: String,
    pub description: String,
    pub max_applications: i32,
    pub rate_limit_per_minute: i32,
    pub requires_identity_verification: bool,
    pub requires_business_agreement: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageMetrics {
    pub total_requests: i64,
    pub successful_requests: i64,
    pub failed_requests: i64,
    pub average_response_time: f64,
    pub requests_per_minute: i64,
    pub rate_limit_utilization: f64,
    pub error_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationUsageSummary {
    pub application_id: Uuid,
    pub application_name: String,
    pub environment: String,
    pub metrics: UsageMetrics,
    pub endpoint_breakdown: Vec<EndpointUsage>,
    pub time_series_data: Vec<TimeSeriesDataPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointUsage {
    pub endpoint: String,
    pub method: String,
    pub request_count: i64,
    pub average_response_time: f64,
    pub error_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesDataPoint {
    pub timestamp: DateTime<Utc>,
    pub request_count: i64,
    pub average_response_time: f64,
    pub error_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookMetrics {
    pub total_deliveries: i64,
    pub successful_deliveries: i64,
    pub failed_deliveries: i64,
    pub success_rate: f64,
    pub average_latency: f64,
    pub pending_deliveries: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminDeveloperAccountList {
    pub accounts: Vec<AdminDeveloperAccountSummary>,
    pub total_count: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminDeveloperAccountSummary {
    pub id: Uuid,
    pub email: String,
    pub full_name: String,
    pub organisation: Option<String>,
    pub country: String,
    pub status_code: String,
    pub access_tier_code: String,
    pub email_verified: bool,
    pub identity_verification_status: String,
    pub application_count: i64,
    pub created_at: DateTime<Utc>,
    pub last_activity: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminProductionAccessQueue {
    pub requests: Vec<AdminProductionAccessRequestSummary>,
    pub total_count: i64,
    pub pending_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminProductionAccessRequestSummary {
    pub id: Uuid,
    pub application_id: Uuid,
    pub application_name: String,
    pub developer_account_id: Uuid,
    pub developer_email: String,
    pub production_use_case: String,
    pub expected_transaction_volume: String,
    pub supported_countries: Vec<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxEnvironment {
    pub wallet_address: String,
    pub wallet_secret: String,
    pub network: String,
    pub initial_balance: String,
    pub api_keys: Vec<SandboxApiKey>,
    pub oauth_clients: Vec<SandboxOAuthClient>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxApiKey {
    pub key_id: Uuid,
    pub key_name: String,
    pub api_key: String,
    pub rate_limit_per_minute: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxOAuthClient {
    pub client_id: String,
    pub client_name: String,
    pub client_secret: String,
    pub redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
}

// Error types
#[derive(Debug, thiserror::Error)]
pub enum DeveloperPortalError {
    #[error("Email already registered")]
    EmailAlreadyRegistered,
    #[error("Account not found")]
    AccountNotFound,
    #[error("Application not found")]
    ApplicationNotFound,
    #[error("API key not found")]
    ApiKeyNotFound,
    #[error("OAuth client not found")]
    OAuthClientNotFound,
    #[error("Webhook configuration not found")]
    WebhookConfigurationNotFound,
    #[error("Production access request not found")]
    ProductionAccessRequestNotFound,
    #[error("Invalid email verification token")]
    InvalidEmailVerificationToken,
    #[error("Email verification token expired")]
    EmailVerificationTokenExpired,
    #[error("Identity verification required")]
    IdentityVerificationRequired,
    #[error("Identity verification already submitted")]
    IdentityVerificationAlreadySubmitted,
    #[error("Maximum applications limit reached")]
    MaximumApplicationsLimitReached,
    #[error("Access tier not found")]
    AccessTierNotFound,
    #[error("Geo-restricted country")]
    GeoRestrictedCountry,
    #[error("Account suspended")]
    AccountSuspended,
    #[error("Invalid environment")]
    InvalidEnvironment,
    #[error("Invalid status")]
    InvalidStatus,
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Invalid UUID: {0}")]
    InvalidUuid(#[from] uuid::Error),
    #[error("Production access request already pending")]
    ProductionAccessRequestAlreadyPending,
    #[error("Not implemented")]
    NotImplemented,
    #[error("Crypto error: {0}")]
    Crypto(String),
}

// ── Sandbox Data Factory ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SandboxTestUser {
    pub id: Uuid,
    pub application_id: Uuid,
    pub external_id: String,
    pub full_name: String,
    pub email: String,
    pub phone: Option<String>,
    pub kyc_status: String,
    pub balance_ngn: rust_decimal::Decimal,
    pub balance_cngn: rust_decimal::Decimal,
    pub stellar_address: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SandboxTestBankAccount {
    pub id: Uuid,
    pub application_id: Uuid,
    pub test_user_id: Option<Uuid>,
    pub account_number: String,
    pub bank_code: String,
    pub bank_name: String,
    pub account_name: String,
    pub currency: String,
    pub is_verified: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SandboxMockTransaction {
    pub id: Uuid,
    pub application_id: Uuid,
    pub test_user_id: Option<Uuid>,
    pub transaction_type: String,
    pub status: String,
    pub amount: rust_decimal::Decimal,
    pub currency: String,
    pub stellar_tx_hash: Option<String>,
    pub reference: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateTestDataRequest {
    /// Number of test users to create (1-50)
    pub user_count: Option<u8>,
    /// Number of mock transactions per user (1-20)
    pub transactions_per_user: Option<u8>,
    /// Initial NGN balance for each test user
    pub initial_balance_ngn: Option<rust_decimal::Decimal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateTestDataResponse {
    pub users_created: usize,
    pub bank_accounts_created: usize,
    pub transactions_created: usize,
    pub users: Vec<SandboxTestUser>,
}

// ── Chaos Injection ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SandboxChaosScenario {
    pub id: Uuid,
    pub application_id: Uuid,
    pub scenario_type: String,
    pub config: serde_json::Value,
    pub target_path_prefix: String,
    pub is_active: bool,
    pub activated_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateChaosScenarioRequest {
    /// One of: http_500, http_429, tx_rejected, latency_ms, network_timeout
    pub scenario_type: String,
    /// For latency_ms: {"delay_ms": 2000}
    /// For http_429: {"retry_after": 60}
    pub config: Option<serde_json::Value>,
    /// Path prefix to intercept, e.g. "/api/v1/onramp"
    pub target_path_prefix: Option<String>,
    /// Auto-expire after N seconds (optional)
    pub expires_in_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivateChaosScenarioRequest {
    pub active: bool,
}

// ── Certification Suite ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SandboxCertificationRun {
    pub id: Uuid,
    pub application_id: Uuid,
    pub status: String,
    pub score: Option<i16>,
    pub passed_tests: i16,
    pub total_tests: i16,
    pub production_gate_met: bool,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SandboxCertificationResult {
    pub id: Uuid,
    pub run_id: Uuid,
    pub test_name: String,
    pub category: String,
    pub passed: bool,
    pub error_message: Option<String>,
    pub duration_ms: Option<i32>,
    pub executed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificationRunSummary {
    pub run: SandboxCertificationRun,
    pub results: Vec<SandboxCertificationResult>,
}
