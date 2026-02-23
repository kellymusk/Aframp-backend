mod api;
mod cache;
mod chains;
mod config;
mod database;
mod error;
mod health;
mod logging;
mod middleware;
mod payments;
mod services;
mod workers;

// Imports
use crate::health::{HealthChecker, HealthStatus};
use crate::logging::init_tracing;
use crate::payments::factory::PaymentProviderFactory;
use crate::payments::types::{
    CustomerContact, Money, PaymentMethod, PaymentRequest as ProviderPaymentRequest, ProviderName,
};
use axum::{
    routing::{get, patch, post},
    Json, Router,
};
use cache::{init_cache_pool, CacheConfig, RedisCache};
use chains::stellar::client::StellarClient;
use chains::stellar::config::StellarConfig;
use database::{init_pool, PoolConfig};
use dotenv::dotenv;
use middleware::logging::{request_logging_middleware, UuidRequestId};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;
use tokio::signal;
use tokio::sync::watch;
use tower::ServiceBuilder;
use tower_http::request_id::{PropagateRequestIdLayer, SetRequestIdLayer};
use tracing::{error, info};
use uuid::Uuid;

/// Graceful shutdown signal handler
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received, starting graceful shutdown");
}

async fn shutdown_signal_with_notify(shutdown_tx: watch::Sender<bool>) {
    shutdown_signal().await;
    let _ = shutdown_tx.send(true);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize advanced tracing
    init_tracing();

    dotenv().ok();
    let skip_externals = std::env::var("SKIP_EXTERNALS")
        .unwrap_or_else(|_| "false".to_string())
        .to_lowercase()
        == "true";

    info!(
        version = env!("CARGO_PKG_VERSION"),
        environment = std::env::var("ENVIRONMENT").unwrap_or_else(|_| "development".to_string()),
        "🚀 Starting Aframp backend service"
    );

    let server_host = std::env::var("SERVER_HOST")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    let server_port = std::env::var("SERVER_PORT")
        .or_else(|_| std::env::var("PORT"))
        .unwrap_or_else(|_| "8000".to_string());

    // Log configuration
    info!(
        host = %server_host,
        port = %server_port,
        "Server configuration loaded"
    );

    // Initialize database connection pool
    let db_pool = if skip_externals {
        info!("⏭️  Skipping database initialization (SKIP_EXTERNALS=true)");
        None
    } else {
        info!("📊 Initializing database connection pool...");
        let database_url =
            std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL not set"))?;
        let db_pool_config = PoolConfig {
            max_connections: std::env::var("DB_MAX_CONNECTIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            min_connections: std::env::var("DB_MIN_CONNECTIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            connection_timeout: Duration::from_secs(
                std::env::var("DB_CONNECTION_TIMEOUT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(30),
            ),
            idle_timeout: Duration::from_secs(
                std::env::var("DB_IDLE_TIMEOUT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(600),
            ),
            max_lifetime: Duration::from_secs(
                std::env::var("DB_MAX_LIFETIME")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1800),
            ),
        };

        let db_pool = init_pool(&database_url, Some(db_pool_config))
            .await
            .map_err(|e| {
                error!("Failed to initialize database pool: {}", e);
                e
            })?;

        info!(
            max_connections = db_pool.options().get_max_connections(),
            "✅ Database connection pool initialized"
        );
        Some(db_pool)
    };

    // Initialize cache connection pool
    let redis_cache = if skip_externals {
        info!("⏭️  Skipping Redis initialization (SKIP_EXTERNALS=true)");
        None
    } else {
        info!("🔄 Initializing Redis cache connection pool...");
        let redis_url =
            std::env::var("REDIS_URL").map_err(|_| anyhow::anyhow!("REDIS_URL not set"))?;

        let cache_config = CacheConfig {
            redis_url: redis_url.clone(),
            max_connections: std::env::var("CACHE_MAX_CONNECTIONS")
                .or_else(|_| std::env::var("REDIS_MAX_CONNECTIONS"))
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            min_idle: std::env::var("REDIS_MIN_IDLE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            connection_timeout: Duration::from_secs(
                std::env::var("REDIS_CONNECTION_TIMEOUT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(5),
            ),
            max_lifetime: Duration::from_secs(
                std::env::var("REDIS_MAX_LIFETIME")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(300),
            ),
            idle_timeout: Duration::from_secs(
                std::env::var("REDIS_IDLE_TIMEOUT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(60),
            ),
            health_check_interval: Duration::from_secs(
                std::env::var("REDIS_HEALTH_CHECK_INTERVAL")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(30),
            ),
        };

        let cache_pool = init_cache_pool(cache_config).await.map_err(|e| {
            error!("Failed to initialize cache pool: {}", e);
            e
        })?;

        let redis_cache = RedisCache::new(cache_pool);
        info!(redis_url = %redis_url, "✅ Cache connection pool initialized");
        Some(redis_cache)
    };

    // Initialize Stellar client
    let stellar_client = if skip_externals {
        info!("⏭️  Skipping Stellar initialization (SKIP_EXTERNALS=true)");
        None
    } else {
        info!("⭐ Initializing Stellar client...");
        let stellar_config = StellarConfig::from_env().map_err(|e| {
            error!("❌ Failed to load Stellar configuration: {}", e);
            e
        })?;

        info!(
            network = ?stellar_config.network,
            timeout_secs = stellar_config.request_timeout.as_secs(),
            max_retries = stellar_config.max_retries,
            "Stellar configuration loaded"
        );

        let stellar_client = StellarClient::new(stellar_config).map_err(|e| {
            error!("❌ Failed to initialize Stellar client: {}", e);
            e
        })?;

        info!("✅ Stellar client initialized successfully");

        // Health check Stellar
        info!("🏥 Performing Stellar health check...");
        let health_status = stellar_client.health_check().await?;
        if health_status.is_healthy {
            info!(
                response_time_ms = health_status.response_time_ms,
                "✅ Stellar Horizon is healthy"
            );
        } else {
            error!(
                error = health_status
                    .error_message
                    .as_deref()
                    .unwrap_or("Unknown error"),
                "❌ Stellar Horizon health check failed"
            );
        }

        // Demo functionality
        info!("🧪 Demo: Testing Stellar functionality");
        let test_address = "GCJRI5CIWK5IU67Q6DGA7QW52JDKRO7JEAHQKFNDUJUPEZGURDBX3LDX";

        match stellar_client.account_exists(test_address).await {
            Ok(exists) => {
                if exists {
                    info!(address = test_address, "✅ Test account exists");
                    match stellar_client.get_account(test_address).await {
                        Ok(account) => {
                            info!(
                                account_id = %account.account_id,
                                sequence = account.sequence,
                                balances = account.balances.len(),
                                "✅ Successfully fetched account details"
                            );
                            for balance in &account.balances {
                                info!(
                                    balance = %balance.balance,
                                    asset_type = %balance.asset_type,
                                    "Account balance"
                                );
                            }
                        }
                        Err(e) => {
                            info!(error = %e, "⚠️  Account exists but couldn't fetch details")
                        }
                    }
                } else {
                    info!(
                        address = test_address,
                        "ℹ️  Test account does not exist (expected)"
                    );
                }
            }
            Err(e) => info!(error = %e, "ℹ️  Error checking account existence (expected for test)"),
        }

        Some(stellar_client)
    };

    // Initialize health checker
    info!("🏥 Initializing health checker...");
    let health_checker =
        HealthChecker::new(db_pool.clone(), redis_cache.clone(), stellar_client.clone());
    info!("✅ Health checker initialized");

    let (worker_shutdown_tx, worker_shutdown_rx) = watch::channel(false);
    let monitor_enabled = std::env::var("TX_MONITOR_ENABLED")
        .unwrap_or_else(|_| "true".to_string())
        .to_lowercase()
        != "false";
    let mut monitor_handle = None;
    if monitor_enabled {
        if let (Some(pool), Some(client)) = (db_pool.clone(), stellar_client.clone()) {
            let monitor_config = workers::transaction_monitor::TransactionMonitorConfig::from_env();
            info!(
                poll_interval_secs = monitor_config.poll_interval.as_secs(),
                pending_timeout_secs = monitor_config.pending_timeout.as_secs(),
                max_retries = monitor_config.max_retries,
                "Starting Stellar transaction monitoring worker"
            );
            let worker = workers::transaction_monitor::TransactionMonitorWorker::new(
                pool,
                client,
                monitor_config,
            );
            monitor_handle = Some(tokio::spawn(worker.run(worker_shutdown_rx)));
        } else {
            info!(
                "Skipping Stellar transaction monitor worker (missing db pool or stellar client)"
            );
        }
    } else {
        info!("Stellar transaction monitor worker disabled (TX_MONITOR_ENABLED=false)");
    }

    // Initialize webhook processor and retry worker
    let webhook_routes = if let Some(pool) = db_pool.clone() {
        let webhook_repo = std::sync::Arc::new(
            database::webhook_repository::WebhookRepository::new(pool.clone()),
        );
        let provider_factory =
            std::sync::Arc::new(PaymentProviderFactory::from_env().unwrap_or_else(|e| {
                error!("Failed to initialize payment provider factory: {}", e);
                panic!("Cannot start without payment providers");
            }));

        // Create orchestrator for webhook processing
        let transaction_repo = std::sync::Arc::new(
            database::transaction_repository::TransactionRepository::new(pool.clone()),
        );
        let orchestrator_config = services::payment_orchestrator::OrchestratorConfig::default();

        // Initialize providers for orchestrator
        let mut providers = Vec::new();
        for provider_name in provider_factory.list_available_providers() {
            if let Ok(provider) = provider_factory.get_provider(provider_name) {
                providers.push(std::sync::Arc::from(provider)
                    as std::sync::Arc<dyn payments::provider::PaymentProvider>);
            }
        }

        let orchestrator =
            std::sync::Arc::new(services::payment_orchestrator::PaymentOrchestrator::new(
                providers,
                transaction_repo,
                orchestrator_config,
            ));

        let webhook_processor =
            std::sync::Arc::new(services::webhook_processor::WebhookProcessor::new(
                webhook_repo,
                provider_factory,
                orchestrator,
            ));

        // Start webhook retry worker
        let webhook_retry_enabled = std::env::var("WEBHOOK_RETRY_ENABLED")
            .unwrap_or_else(|_| "true".to_string())
            .to_lowercase()
            != "false";

        if webhook_retry_enabled {
            let retry_worker = workers::webhook_retry::WebhookRetryWorker::new(
                webhook_processor.clone(),
                60, // Check every 60 seconds
            );
            tokio::spawn(async move {
                retry_worker.run().await;
            });
            info!("✅ Webhook retry worker started");
        }

        let webhook_state = api::webhooks::WebhookState {
            processor: webhook_processor,
        };

        Router::new()
            .route("/webhooks/{provider}", post(api::webhooks::handle_webhook))
            .with_state(std::sync::Arc::new(webhook_state))
    } else {
        info!("⏭️  Skipping webhook routes (no database)");
        Router::new()
    };

    // Create the application router with logging middleware
    info!("🛣️  Setting up application routes...");

    // Setup onramp routes (quote service)
    let onramp_routes = if let (Some(pool), Some(cache), Some(client)) =
        (db_pool.clone(), redis_cache.clone(), stellar_client.clone())
    {
        let cngn_issuer = std::env::var("CNGN_ISSUER_ADDRESS")
            .or_else(|_| std::env::var("CNGN_ISSUER_MAINNET"))
            .unwrap_or_else(|_| "GXXXXDEFAULTISSUERXXXX".to_string());

        let rate_repo =
            database::exchange_rate_repository::ExchangeRateRepository::new(pool.clone());
        let fee_repo =
            database::fee_structure_repository::FeeStructureRepository::new(pool.clone());
        let fee_service =
            std::sync::Arc::new(services::fee_structure::FeeStructureService::new(fee_repo));

        let exchange_rate_service = std::sync::Arc::new(
            services::exchange_rate::ExchangeRateService::new(
                rate_repo,
                services::exchange_rate::ExchangeRateServiceConfig::default(),
            )
            .with_cache(cache.clone())
            .add_provider(std::sync::Arc::new(
                services::rate_providers::FixedRateProvider::new(),
            ))
            .with_fee_service(fee_service.clone()),
        );

        let quote_service = std::sync::Arc::new(services::onramp_quote::OnrampQuoteService::new(
            exchange_rate_service,
            fee_service,
            client,
            cache,
            cngn_issuer,
        ));

        Router::new()
            .route("/api/onramp/quote", post(create_onramp_quote))
            .with_state(quote_service)
    } else {
        Router::new()
    };

    // Setup wallet routes with balance service
    let wallet_routes =
        if let (Some(client), Some(cache)) = (stellar_client.clone(), redis_cache.clone()) {
            let cngn_issuer = std::env::var("CNGN_ISSUER_ADDRESS")
                .unwrap_or_else(|_| "GXXXXDEFAULTISSUERXXXX".to_string());

            let balance_service = std::sync::Arc::new(services::balance::BalanceService::new(
                client,
                cache,
                cngn_issuer,
            ));

            let wallet_state = api::wallet::WalletState { balance_service };

            Router::new()
                .route("/api/wallet/balance", get(api::wallet::get_balance))
                .with_state(wallet_state)
        } else {
            Router::new()
        };

    // Bill payment providers routes (public endpoint - no auth required)
    let bills_routes = Router::new().route("/api/bills/providers", get(api::bills::get_providers));

    let app = Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .route("/health/ready", get(readiness))
        .route("/health/live", get(liveness))
        .route("/api/stellar/account/{address}", get(get_stellar_account))
        .route(
            "/api/trustlines/operations",
            post(create_trustline_operation),
        )
        .route(
            "/api/trustlines/operations/{id}",
            patch(update_trustline_operation_status),
        )
        .route(
            "/api/trustlines/operations/wallet/{address}",
            get(list_trustline_operations_by_wallet),
        )
        .route("/api/fees/calculate", post(calculate_fee))
        .route("/api/cngn/trustlines/check", post(check_cngn_trustline))
        .route(
            "/api/cngn/trustlines/preflight",
            post(preflight_cngn_trustline),
        )
        .route("/api/cngn/trustlines/build", post(build_cngn_trustline))
        .route("/api/cngn/trustlines/submit", post(submit_cngn_trustline))
        .route(
            "/api/cngn/trustlines/retry/{id}",
            post(retry_cngn_trustline),
        )
        .route("/api/cngn/payments/build", post(build_cngn_payment))
        .route("/api/cngn/payments/sign", post(sign_cngn_payment))
        .route("/api/cngn/payments/submit", post(submit_cngn_payment))
        .route("/api/payments/initiate", post(initiate_payment))
        .merge(onramp_routes)
        .merge(wallet_routes)
        .merge(webhook_routes)
        .merge(bills_routes)
        .with_state(AppState {
            db_pool,
            redis_cache,
            stellar_client,
            health_checker,
        })
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(UuidRequestId))
                .layer(axum::middleware::from_fn(request_logging_middleware))
                .layer(PropagateRequestIdLayer::x_request_id()),
        );

    info!("✅ Routes configured");

    // Run the server with graceful shutdown
    let addr: SocketAddr = format!("{}:{}", server_host, server_port).parse()?;

    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        error!("❌ Failed to bind to address {}: {}", addr, e);
        e
    })?;

    // Print a prominent banner with server information
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║                                                              ║");
    println!("║          🚀 AFRAMP BACKEND SERVER IS RUNNING 🚀             ║");
    println!("║                                                              ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║                                                              ║");
    println!(
        "║  🌐 Server Address:  http://{}                    ║",
        addr
    );
    println!(
        "║  📡 Port:            {}                                  ║",
        server_port
    );
    println!(
        "║  🏠 Host:            {}                            ║",
        server_host
    );
    println!("║                                                              ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  📡 AVAILABLE ENDPOINTS:                                     ║");
    println!("║                                                              ║");
    println!("║  GET  /                          - Root endpoint            ║");
    println!("║  GET  /health                    - Health check             ║");
    println!("║  GET  /health/ready              - Readiness probe          ║");
    println!("║  GET  /health/live               - Liveness probe           ║");
    println!("║  GET  /api/stellar/account/{{address}} - Stellar account    ║");
    println!("║                                                              ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║                                                              ║");
    println!("║  💡 Try it out:                                              ║");
    println!(
        "║     curl http://{}                                ║",
        addr
    );
    println!("║     curl http://{}/health                        ║", addr);
    println!("║                                                              ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    info!(
        address = %addr,
        port = %server_port,
        "🚀 Server listening on http://{}",
        addr
    );
    info!("✅ Server is ready to accept connections");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal_with_notify(worker_shutdown_tx.clone()))
        .await
        .unwrap();

    let _ = worker_shutdown_tx.send(true);
    if let Some(handle) = monitor_handle {
        if let Err(e) = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
            error!(error = %e, "Timed out waiting for monitor worker shutdown");
        }
    }

    info!("👋 Server shutdown complete");

    Ok(())
}

// Application state
#[derive(Clone)]
struct AppState {
    db_pool: Option<sqlx::PgPool>,
    redis_cache: Option<RedisCache>,
    stellar_client: Option<StellarClient>,
    health_checker: HealthChecker,
}

// Handlers
async fn root() -> &'static str {
    info!("📍 Root endpoint accessed");
    "Welcome to Aframp Backend API"
}

async fn health(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<Json<HealthStatus>, (axum::http::StatusCode, String)> {
    info!("🏥 Health check requested");
    let health_status = state.health_checker.check_health().await;

    // Return 503 if any component is unhealthy
    if matches!(health_status.status, crate::health::HealthState::Unhealthy) {
        error!("❌ Health check failed - service unhealthy");
        Err((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Service Unavailable".to_string(),
        ))
    } else {
        info!("✅ Health check passed");
        Ok(Json(health_status))
    }
}

/// Readiness probe - checks if the service is ready to accept traffic
async fn readiness(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<Json<HealthStatus>, (axum::http::StatusCode, String)> {
    info!("🔍 Readiness probe requested");
    // Readiness checks all dependencies
    let result = health(axum::extract::State(state)).await;
    if result.is_ok() {
        info!("✅ Readiness check passed");
    } else {
        error!("❌ Readiness check failed");
    }
    result
}

/// Liveness probe - checks if the service is alive (basic check)
async fn liveness() -> Result<&'static str, (axum::http::StatusCode, String)> {
    info!("💓 Liveness probe requested");
    // Liveness just checks if the service is running
    info!("✅ Liveness check passed");
    Ok("OK")
}

async fn get_stellar_account(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(address): axum::extract::Path<String>,
) -> Result<String, (axum::http::StatusCode, String)> {
    info!(address = %address, "🔍 Stellar account lookup requested");

    let stellar_client = match state.stellar_client.as_ref() {
        Some(client) => client,
        None => {
            return Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Stellar client disabled by configuration".to_string(),
            ))
        }
    };

    match stellar_client.account_exists(&address).await {
        Ok(exists) => {
            if exists {
                info!(address = %address, "✅ Account exists, fetching details");
                match stellar_client.get_account(&address).await {
                    Ok(account) => {
                        info!(
                            address = %address,
                            balances = account.balances.len(),
                            "✅ Account details fetched successfully"
                        );
                        Ok(format!(
                            "Account: {}, Balances: {}",
                            account.account_id,
                            account.balances.len()
                        ))
                    }
                    Err(e) => {
                        error!(address = %address, error = %e, "❌ Failed to fetch account details");
                        Err((
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Failed to fetch account: {}", e),
                        ))
                    }
                }
            } else {
                info!(address = %address, "ℹ️  Account not found");
                Err((
                    axum::http::StatusCode::NOT_FOUND,
                    "Account not found".to_string(),
                ))
            }
        }
        Err(e) => {
            error!(address = %address, error = %e, "❌ Error checking account existence");
            Err((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Error checking account: {}", e),
            ))
        }
    }
}

#[derive(Debug, Deserialize)]
struct TrustlineOperationRequest {
    wallet_address: String,
    asset_code: String,
    issuer: Option<String>,
    operation_type: TrustlineOperationType,
    status: TrustlineOperationStatus,
    transaction_hash: Option<String>,
    error_message: Option<String>,
    metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct TrustlineOperationStatusUpdate {
    status: TrustlineOperationStatus,
    transaction_hash: Option<String>,
    error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TrustlineOperationQuery {
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TrustlineOperationType {
    Create,
    Update,
    Remove,
}

impl TrustlineOperationType {
    fn as_str(&self) -> &'static str {
        match self {
            TrustlineOperationType::Create => "create",
            TrustlineOperationType::Update => "update",
            TrustlineOperationType::Remove => "remove",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TrustlineOperationStatus {
    Pending,
    Completed,
    Failed,
}

impl TrustlineOperationStatus {
    fn as_str(&self) -> &'static str {
        match self {
            TrustlineOperationStatus::Pending => "pending",
            TrustlineOperationStatus::Completed => "completed",
            TrustlineOperationStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Deserialize)]
struct FeeCalculationRequest {
    fee_type: FeeType,
    amount: String,
    currency: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FeeType {
    Onramp,
    Offramp,
    BillPayment,
    Exchange,
    Transfer,
}

impl FeeType {
    fn as_str(&self) -> &'static str {
        match self {
            FeeType::Onramp => "onramp",
            FeeType::Offramp => "offramp",
            FeeType::BillPayment => "bill_payment",
            FeeType::Exchange => "exchange",
            FeeType::Transfer => "transfer",
        }
    }
}

#[derive(Debug, Serialize)]
struct FeeCalculationResponse {
    fee: String,
    rate_bps: i32,
    flat_fee: String,
    min_fee: Option<String>,
    max_fee: Option<String>,
    currency: Option<String>,
    structure_id: String,
}

#[derive(Debug, Deserialize)]
struct TrustlineAccountRequest {
    account_id: String,
}

#[derive(Debug, Serialize)]
struct TrustlineVerificationResponse {
    verified: bool,
}

#[derive(Debug, Deserialize)]
struct CngnTrustlineBuildRequest {
    account_id: String,
    limit: Option<String>,
    fee_stroops: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct CngnTrustlineSubmitRequest {
    signed_envelope_xdr: String,
    account_id: Option<String>,
    operation_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct CngnTrustlineBuildResponse {
    draft: crate::chains::stellar::trustline::UnsignedTrustlineTransaction,
    operation_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct CngnTrustlineSubmitResponse {
    horizon_response: serde_json::Value,
    operation_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct CngnPaymentBuildRequest {
    source: String,
    destination: String,
    amount: String,
    memo: Option<crate::chains::stellar::payment::CngnMemo>,
    fee_stroops: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct CngnPaymentSignRequest {
    draft: crate::chains::stellar::payment::CngnPaymentDraft,
    secret_seed: String,
}

#[derive(Debug, Deserialize)]
struct CngnPaymentSubmitRequest {
    signed_envelope_xdr: String,
    transaction_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct CngnPaymentBuildResponse {
    draft: crate::chains::stellar::payment::CngnPaymentDraft,
    transaction_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct CngnPaymentSubmitResponse {
    horizon_response: serde_json::Value,
    transaction_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InitiatePaymentApiRequest {
    amount: String,
    currency: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    payment_method: Option<String>,
    callback_url: Option<String>,
    transaction_reference: String,
    metadata: Option<serde_json::Value>,
    provider: Option<String>,
}

async fn create_trustline_operation(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<TrustlineOperationRequest>,
) -> Result<
    Json<crate::database::trustline_operation_repository::TrustlineOperation>,
    (
        axum::http::StatusCode,
        Json<crate::middleware::error::ErrorResponse>,
    ),
> {
    let request_id = crate::middleware::error::get_request_id_from_headers(&headers);
    let pool = match state.db_pool.as_ref() {
        Some(pool) => pool,
        None => {
            return Err(crate::middleware::error::json_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Database disabled by configuration",
                request_id,
            ))
        }
    };

    if payload.wallet_address.trim().is_empty() {
        return Err(crate::middleware::error::json_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "wallet_address is required",
            request_id,
        ));
    }
    if payload.asset_code.trim().is_empty() {
        return Err(crate::middleware::error::json_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "asset_code is required",
            request_id,
        ));
    }

    let repo = crate::database::trustline_operation_repository::TrustlineOperationRepository::new(
        pool.clone(),
    );
    let service = crate::services::trustline_operation::TrustlineOperationService::new(repo);

    let input = crate::services::trustline_operation::TrustlineOperationInput {
        wallet_address: payload.wallet_address,
        asset_code: payload.asset_code,
        issuer: payload.issuer,
        operation_type: payload.operation_type.as_str().to_string(),
        status: payload.status.as_str().to_string(),
        transaction_hash: payload.transaction_hash,
        error_message: payload.error_message,
        metadata: payload.metadata.unwrap_or_else(|| serde_json::json!({})),
    };

    let result = match payload.operation_type {
        TrustlineOperationType::Create => service.record_create(input).await,
        TrustlineOperationType::Update => service.record_update(input).await,
        TrustlineOperationType::Remove => service.record_remove(input).await,
    };

    result.map(Json).map_err(|e| {
        crate::middleware::error::json_error_response(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
            request_id,
        )
    })
}

async fn initiate_payment(
    headers: axum::http::HeaderMap,
    Json(payload): Json<InitiatePaymentApiRequest>,
) -> Result<
    Json<crate::payments::types::PaymentResponse>,
    (
        axum::http::StatusCode,
        Json<crate::middleware::error::ErrorResponse>,
    ),
> {
    let request_id = crate::middleware::error::get_request_id_from_headers(&headers);

    if payload.transaction_reference.trim().is_empty() {
        return Err(crate::middleware::error::json_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "transaction_reference is required",
            request_id,
        ));
    }
    if payload.email.as_deref().unwrap_or("").trim().is_empty() {
        return Err(crate::middleware::error::json_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "email is required for payment initialization",
            request_id,
        ));
    }

    let payment_method = match payload
        .payment_method
        .as_deref()
        .unwrap_or("card")
        .trim()
        .to_lowercase()
        .as_str()
    {
        "card" => PaymentMethod::Card,
        "bank_transfer" | "bank" => PaymentMethod::BankTransfer,
        "mobile_money" => PaymentMethod::MobileMoney,
        "ussd" => PaymentMethod::Ussd,
        "wallet" => PaymentMethod::Wallet,
        _ => PaymentMethod::Other,
    };

    let provider_request = ProviderPaymentRequest {
        amount: Money {
            amount: payload.amount,
            currency: payload.currency.unwrap_or_else(|| "NGN".to_string()),
        },
        customer: CustomerContact {
            email: payload.email,
            phone: payload.phone,
        },
        payment_method,
        callback_url: payload.callback_url,
        transaction_reference: payload.transaction_reference,
        metadata: payload.metadata,
    };

    let factory = PaymentProviderFactory::from_env().map_err(|e| {
        crate::middleware::error::json_error_response(
            axum::http::StatusCode::from_u16(e.http_status_code())
                .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
            e.user_message(),
            request_id.clone(),
        )
    })?;

    let provider = match payload.provider {
        Some(provider_name) => {
            let provider = ProviderName::from_str(&provider_name).map_err(|e| {
                crate::middleware::error::json_error_response(
                    axum::http::StatusCode::from_u16(e.http_status_code())
                        .unwrap_or(axum::http::StatusCode::BAD_REQUEST),
                    e.user_message(),
                    request_id.clone(),
                )
            })?;
            factory.get_provider(provider)
        }
        None => factory.get_default_provider(),
    }
    .map_err(|e| {
        crate::middleware::error::json_error_response(
            axum::http::StatusCode::from_u16(e.http_status_code())
                .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
            e.user_message(),
            request_id.clone(),
        )
    })?;

    let response = provider
        .initiate_payment(provider_request)
        .await
        .map_err(|e| {
            crate::middleware::error::json_error_response(
                axum::http::StatusCode::from_u16(e.http_status_code())
                    .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
                e.user_message(),
                request_id.clone(),
            )
        })?;

    Ok(Json(response))
}

async fn update_trustline_operation_status(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<TrustlineOperationStatusUpdate>,
) -> Result<
    Json<crate::database::trustline_operation_repository::TrustlineOperation>,
    (
        axum::http::StatusCode,
        Json<crate::middleware::error::ErrorResponse>,
    ),
> {
    let request_id = crate::middleware::error::get_request_id_from_headers(&headers);
    let pool = match state.db_pool.as_ref() {
        Some(pool) => pool,
        None => {
            return Err(crate::middleware::error::json_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Database disabled by configuration",
                request_id,
            ))
        }
    };

    let uuid = Uuid::parse_str(&id).map_err(|e| {
        crate::middleware::error::json_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            format!("Invalid UUID: {}", e),
            request_id.clone(),
        )
    })?;

    let repo = crate::database::trustline_operation_repository::TrustlineOperationRepository::new(
        pool.clone(),
    );
    let service = crate::services::trustline_operation::TrustlineOperationService::new(repo);

    service
        .update_status(
            uuid,
            payload.status.as_str(),
            payload.transaction_hash.as_deref(),
            payload.error_message.as_deref(),
        )
        .await
        .map(Json)
        .map_err(|e| {
            crate::middleware::error::json_error_response(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                e.to_string(),
                request_id.clone(),
            )
        })
}

async fn list_trustline_operations_by_wallet(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(address): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(query): axum::extract::Query<TrustlineOperationQuery>,
) -> Result<
    Json<Vec<crate::database::trustline_operation_repository::TrustlineOperation>>,
    (
        axum::http::StatusCode,
        Json<crate::middleware::error::ErrorResponse>,
    ),
> {
    let request_id = crate::middleware::error::get_request_id_from_headers(&headers);
    let pool = match state.db_pool.as_ref() {
        Some(pool) => pool,
        None => {
            return Err(crate::middleware::error::json_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Database disabled by configuration",
                request_id,
            ))
        }
    };

    if address.trim().is_empty() {
        return Err(crate::middleware::error::json_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "wallet address is required",
            request_id,
        ));
    }

    let repo = crate::database::trustline_operation_repository::TrustlineOperationRepository::new(
        pool.clone(),
    );

    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    repo.find_by_wallet(&address, limit)
        .await
        .map(Json)
        .map_err(|e| {
            crate::middleware::error::json_error_response(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                e.to_string(),
                request_id,
            )
        })
}

async fn create_onramp_quote(
    axum::extract::State(quote_service): axum::extract::State<
        std::sync::Arc<services::onramp_quote::OnrampQuoteService>,
    >,
    headers: axum::http::HeaderMap,
    Json(payload): Json<services::onramp_quote::OnrampQuoteRequest>,
) -> Result<
    Json<services::onramp_quote::OnrampQuoteResponse>,
    (
        axum::http::StatusCode,
        Json<middleware::error::ErrorResponse>,
    ),
> {
    let request_id = middleware::error::get_request_id_from_headers(&headers);

    quote_service
        .create_quote(payload)
        .await
        .map(Json)
        .map_err(|e| app_error_response(e, request_id))
}

async fn calculate_fee(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<FeeCalculationRequest>,
) -> Result<
    Json<FeeCalculationResponse>,
    (
        axum::http::StatusCode,
        Json<crate::middleware::error::ErrorResponse>,
    ),
> {
    let request_id = crate::middleware::error::get_request_id_from_headers(&headers);
    let pool = match state.db_pool.as_ref() {
        Some(pool) => pool,
        None => {
            return Err(crate::middleware::error::json_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Database disabled by configuration",
                request_id,
            ))
        }
    };

    let repo = crate::database::fee_structure_repository::FeeStructureRepository::new(pool.clone());
    let service = crate::services::fee_structure::FeeStructureService::new(repo);

    let amount = crate::services::fee_structure::parse_amount(&payload.amount);
    if amount <= bigdecimal::BigDecimal::from(0) {
        return Err(crate::middleware::error::json_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "amount must be greater than 0",
            request_id,
        ));
    }

    let result = service
        .calculate_fee(crate::services::fee_structure::FeeCalculationInput {
            fee_type: payload.fee_type.as_str().to_string(),
            amount,
            currency: payload.currency,
            at_time: None,
        })
        .await
        .map_err(|e| {
            crate::middleware::error::json_error_response(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                e.to_string(),
                request_id.clone(),
            )
        })?;

    match result {
        Some(calc) => Ok(Json(FeeCalculationResponse {
            fee: calc.fee.to_string(),
            rate_bps: calc.rate_bps,
            flat_fee: calc.flat_fee.to_string(),
            min_fee: calc.min_fee.map(|v| v.to_string()),
            max_fee: calc.max_fee.map(|v| v.to_string()),
            currency: calc.currency,
            structure_id: calc.structure_id.to_string(),
        })),
        None => Err(crate::middleware::error::json_error_response(
            axum::http::StatusCode::NOT_FOUND,
            "No active fee structure found",
            request_id.clone(),
        )),
    }
}

fn app_error_response(
    err: crate::error::AppError,
    request_id: Option<String>,
) -> (
    axum::http::StatusCode,
    Json<crate::middleware::error::ErrorResponse>,
) {
    let err = match request_id {
        Some(req_id) => err.with_request_id(req_id),
        None => err,
    };
    let status = axum::http::StatusCode::from_u16(err.status_code())
        .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        Json(crate::middleware::error::ErrorResponse::from_app_error(
            &err,
        )),
    )
}

async fn check_cngn_trustline(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<TrustlineAccountRequest>,
) -> Result<
    Json<crate::chains::stellar::trustline::TrustlineStatus>,
    (
        axum::http::StatusCode,
        Json<crate::middleware::error::ErrorResponse>,
    ),
> {
    let request_id = crate::middleware::error::get_request_id_from_headers(&headers);
    let stellar_client = match state.stellar_client.as_ref() {
        Some(client) => client,
        None => {
            return Err(crate::middleware::error::json_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Stellar client disabled by configuration",
                request_id,
            ))
        }
    };

    if payload.account_id.trim().is_empty() {
        return Err(crate::middleware::error::json_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "account_id is required",
            request_id,
        ));
    }

    let manager =
        crate::chains::stellar::trustline::CngnTrustlineManager::new(stellar_client.clone());
    manager
        .check_trustline(&payload.account_id)
        .await
        .map(Json)
        .map_err(|e| app_error_response(e.into(), request_id))
}

async fn preflight_cngn_trustline(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<TrustlineAccountRequest>,
) -> Result<
    Json<crate::chains::stellar::trustline::TrustlinePreflight>,
    (
        axum::http::StatusCode,
        Json<crate::middleware::error::ErrorResponse>,
    ),
> {
    let request_id = crate::middleware::error::get_request_id_from_headers(&headers);
    let stellar_client = match state.stellar_client.as_ref() {
        Some(client) => client,
        None => {
            return Err(crate::middleware::error::json_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Stellar client disabled by configuration",
                request_id,
            ))
        }
    };

    if payload.account_id.trim().is_empty() {
        return Err(crate::middleware::error::json_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "account_id is required",
            request_id,
        ));
    }

    let manager =
        crate::chains::stellar::trustline::CngnTrustlineManager::new(stellar_client.clone());
    manager
        .preflight_trustline_creation(&payload.account_id)
        .await
        .map(Json)
        .map_err(|e| app_error_response(e.into(), request_id))
}

async fn build_cngn_trustline(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<CngnTrustlineBuildRequest>,
) -> Result<
    Json<CngnTrustlineBuildResponse>,
    (
        axum::http::StatusCode,
        Json<crate::middleware::error::ErrorResponse>,
    ),
> {
    let request_id = crate::middleware::error::get_request_id_from_headers(&headers);
    let stellar_client = match state.stellar_client.as_ref() {
        Some(client) => client,
        None => {
            return Err(crate::middleware::error::json_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Stellar client disabled by configuration",
                request_id,
            ))
        }
    };

    if payload.account_id.trim().is_empty() {
        return Err(crate::middleware::error::json_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "account_id is required",
            request_id,
        ));
    }

    let manager =
        crate::chains::stellar::trustline::CngnTrustlineManager::new(stellar_client.clone());
    let draft = manager
        .build_create_trustline_transaction(
            &payload.account_id,
            payload.limit.as_deref(),
            payload.fee_stroops,
        )
        .await
        .map_err(|e| app_error_response(e.into(), request_id.clone()))?;

    let mut operation_id = None;
    if let Some(pool) = state.db_pool.as_ref() {
        let repo =
            crate::database::trustline_operation_repository::TrustlineOperationRepository::new(
                pool.clone(),
            );
        let operation = repo
            .create_operation(
                &draft.account_id,
                &draft.asset_code,
                Some(&draft.issuer),
                "create",
                "pending",
                Some(&draft.transaction_hash),
                None,
                serde_json::json!({
                    "unsigned_envelope_xdr": draft.unsigned_envelope_xdr,
                    "sequence": draft.sequence,
                    "fee_stroops": draft.fee_stroops,
                    "limit": draft.limit
                }),
            )
            .await
            .map_err(|e| {
                crate::middleware::error::json_error_response(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to log trustline operation: {}", e),
                    request_id.clone(),
                )
            })?;
        operation_id = Some(operation.id);
    }

    Ok(Json(CngnTrustlineBuildResponse {
        draft,
        operation_id,
    }))
}

async fn submit_cngn_trustline(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<CngnTrustlineSubmitRequest>,
) -> Result<
    Json<CngnTrustlineSubmitResponse>,
    (
        axum::http::StatusCode,
        Json<crate::middleware::error::ErrorResponse>,
    ),
> {
    let request_id = crate::middleware::error::get_request_id_from_headers(&headers);
    let stellar_client = match state.stellar_client.as_ref() {
        Some(client) => client,
        None => {
            return Err(crate::middleware::error::json_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Stellar client disabled by configuration",
                request_id,
            ))
        }
    };

    if payload.signed_envelope_xdr.trim().is_empty() {
        return Err(crate::middleware::error::json_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "signed_envelope_xdr is required",
            request_id,
        ));
    }

    let manager =
        crate::chains::stellar::trustline::CngnTrustlineManager::new(stellar_client.clone());
    let result = manager
        .submit_signed_trustline_xdr(&payload.signed_envelope_xdr)
        .await;

    match result {
        Ok(horizon_response) => {
            if let (Some(pool), Some(op_id)) = (state.db_pool.as_ref(), payload.operation_id) {
                let repo = crate::database::trustline_operation_repository::TrustlineOperationRepository::new(pool.clone());
                let tx_hash = horizon_response.get("hash").and_then(|v| v.as_str());
                let _ = repo.update_status(op_id, "completed", tx_hash, None).await;
            }
            Ok(Json(CngnTrustlineSubmitResponse {
                horizon_response,
                operation_id: payload.operation_id,
            }))
        }
        Err(e) => {
            if let (Some(pool), Some(op_id)) = (state.db_pool.as_ref(), payload.operation_id) {
                let repo = crate::database::trustline_operation_repository::TrustlineOperationRepository::new(pool.clone());
                let _ = repo
                    .update_status(op_id, "failed", None, Some(&e.to_string()))
                    .await;
            }
            Err(app_error_response(e.into(), request_id))
        }
    }
}

async fn retry_cngn_trustline(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    headers: axum::http::HeaderMap,
) -> Result<
    Json<crate::database::trustline_operation_repository::TrustlineOperation>,
    (
        axum::http::StatusCode,
        Json<crate::middleware::error::ErrorResponse>,
    ),
> {
    let request_id = crate::middleware::error::get_request_id_from_headers(&headers);
    let pool = match state.db_pool.as_ref() {
        Some(pool) => pool,
        None => {
            return Err(crate::middleware::error::json_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Database disabled by configuration",
                request_id,
            ))
        }
    };

    let repo = crate::database::trustline_operation_repository::TrustlineOperationRepository::new(
        pool.clone(),
    );
    repo.update_status(id, "pending", None, None)
        .await
        .map(Json)
        .map_err(|e| {
            crate::middleware::error::json_error_response(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                e.to_string(),
                request_id,
            )
        })
}

async fn build_cngn_payment(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<CngnPaymentBuildRequest>,
) -> Result<
    Json<CngnPaymentBuildResponse>,
    (
        axum::http::StatusCode,
        Json<crate::middleware::error::ErrorResponse>,
    ),
> {
    let request_id = crate::middleware::error::get_request_id_from_headers(&headers);
    let stellar_client = match state.stellar_client.as_ref() {
        Some(client) => client,
        None => {
            return Err(crate::middleware::error::json_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Stellar client disabled by configuration",
                request_id,
            ))
        }
    };

    if payload.source.trim().is_empty()
        || payload.destination.trim().is_empty()
        || payload.amount.trim().is_empty()
    {
        return Err(crate::middleware::error::json_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "source, destination and amount are required",
            request_id,
        ));
    }

    let builder = crate::chains::stellar::payment::CngnPaymentBuilder::new(stellar_client.clone());
    let draft = builder
        .build_payment(
            &payload.source,
            &payload.destination,
            &payload.amount,
            payload
                .memo
                .unwrap_or(crate::chains::stellar::payment::CngnMemo::None),
            payload.fee_stroops,
        )
        .await
        .map_err(|e| app_error_response(e.into(), request_id.clone()))?;

    let mut transaction_id = None;
    if let Some(pool) = state.db_pool.as_ref() {
        let repo =
            crate::database::transaction_repository::TransactionRepository::new(pool.clone());

        // Parse amounts as BigDecimal
        use sqlx::types::BigDecimal;
        use std::str::FromStr;
        let amount_bd =
            BigDecimal::from_str(&payload.amount).unwrap_or_else(|_| BigDecimal::from(0));

        // Get asset code from draft (cNGN or XLM)
        let asset_code = if draft.asset_code.is_empty() {
            "XLM".to_string()
        } else {
            draft.asset_code.clone()
        };

        let tx = repo
            .create_transaction(
                &payload.source,
                "payment",
                &asset_code,
                &asset_code,
                amount_bd.clone(),
                amount_bd.clone(),
                BigDecimal::from(0), // cngn_amount
                "pending",
                None, // payment_provider
                None, // payment_reference
                serde_json::json!({
                    "asset_code": draft.asset_code,
                    "asset_issuer": draft.asset_issuer,
                    "destination": payload.destination,
                    "memo": draft.memo,
                    "stellar_tx_hash": draft.transaction_hash,
                    "unsigned_envelope_xdr": draft.unsigned_envelope_xdr
                }),
            )
            .await
            .map_err(|e| {
                crate::middleware::error::json_error_response(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to log payment transaction: {}", e),
                    request_id.clone(),
                )
            })?;
        transaction_id = Some(tx.transaction_id.to_string());
    }

    Ok(Json(CngnPaymentBuildResponse {
        draft,
        transaction_id,
    }))
}

async fn sign_cngn_payment(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<CngnPaymentSignRequest>,
) -> Result<
    Json<crate::chains::stellar::payment::SignedCngnPayment>,
    (
        axum::http::StatusCode,
        Json<crate::middleware::error::ErrorResponse>,
    ),
> {
    let request_id = crate::middleware::error::get_request_id_from_headers(&headers);
    let stellar_client = match state.stellar_client.as_ref() {
        Some(client) => client,
        None => {
            return Err(crate::middleware::error::json_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Stellar client disabled by configuration",
                request_id,
            ))
        }
    };

    let builder = crate::chains::stellar::payment::CngnPaymentBuilder::new(stellar_client.clone());
    builder
        .sign_payment(payload.draft, &payload.secret_seed)
        .map(Json)
        .map_err(|e| app_error_response(e.into(), request_id))
}

async fn submit_cngn_payment(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<CngnPaymentSubmitRequest>,
) -> Result<
    Json<CngnPaymentSubmitResponse>,
    (
        axum::http::StatusCode,
        Json<crate::middleware::error::ErrorResponse>,
    ),
> {
    let request_id = crate::middleware::error::get_request_id_from_headers(&headers);
    let stellar_client = match state.stellar_client.as_ref() {
        Some(client) => client,
        None => {
            return Err(crate::middleware::error::json_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Stellar client disabled by configuration",
                request_id,
            ))
        }
    };

    if payload.signed_envelope_xdr.trim().is_empty() {
        return Err(crate::middleware::error::json_error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "signed_envelope_xdr is required",
            request_id,
        ));
    }

    let builder = crate::chains::stellar::payment::CngnPaymentBuilder::new(stellar_client.clone());
    let submit_result = builder
        .submit_signed_payment(&payload.signed_envelope_xdr)
        .await;

    match submit_result {
        Ok(horizon_response) => {
            if let (Some(pool), Some(tx_id)) =
                (state.db_pool.as_ref(), payload.transaction_id.as_deref())
            {
                let repo = crate::database::transaction_repository::TransactionRepository::new(
                    pool.clone(),
                );
                let submitted_hash = horizon_response
                    .get("hash")
                    .and_then(|v| v.as_str())
                    .map(|v| v.to_string());
                let mut metadata = serde_json::json!({
                    "submitted_at": chrono::Utc::now().to_rfc3339(),
                    "horizon_response": horizon_response.clone(),
                });
                if let Some(hash) = submitted_hash {
                    metadata["submitted_hash"] = serde_json::json!(hash);
                }
                let _ = repo
                    .update_status_with_metadata(tx_id, "processing", metadata)
                    .await;
            }
            Ok(Json(CngnPaymentSubmitResponse {
                horizon_response,
                transaction_id: payload.transaction_id,
            }))
        }
        Err(e) => {
            if let (Some(pool), Some(tx_id)) =
                (state.db_pool.as_ref(), payload.transaction_id.as_deref())
            {
                let repo = crate::database::transaction_repository::TransactionRepository::new(
                    pool.clone(),
                );
                let _ = repo.update_status(tx_id, "failed").await;
            }
            Err(app_error_response(e.into(), request_id))
        }
    }
}
