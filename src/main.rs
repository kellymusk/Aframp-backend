mod api;
mod api_keys;
mod audit;
mod auth;
mod cache;
mod chains;
mod config;
mod config_validation;
mod database;
mod ddos;
mod developer_portal;
mod error;
mod health;
mod logging;
mod metrics;
mod middleware;
mod mtls;
mod oauth;
mod payments;
mod pentest;
mod recurring;
mod services;
mod telemetry;
mod workers;

// Imports
use std::sync::Arc;
use crate::config::AppConfig;
use crate::health::{HealthChecker, HealthStatus};
use crate::telemetry::tracer::{init_tracer, shutdown_tracer};    // Issue #104
use crate::payments::factory::PaymentProviderFactory;
use crate::payments::types::{
    CustomerContact, Money, PaymentMethod, PaymentRequest as ProviderPaymentRequest, ProviderName,
};
use axum::{
    routing::{delete, get, patch, post},
    Json, Router,
};
use cache::{init_cache_pool, build_multi_level_cache, CacheConfig, RedisCache};
use cache::warmer::{warm_caches, WarmingState};
use chains::stellar::client::StellarClient;
use chains::stellar::config::StellarConfig;
use database::{init_pool, PoolConfig};
use dotenv::dotenv;
use middleware::logging::{request_logging_middleware, UuidRequestId};
use middleware::metrics::metrics_middleware;
use middleware::cors::{cors_middleware, CorsConfig};
use middleware::security::security_headers_middleware;
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
    // -------------------------------------------------------------------------
    // 1. Load application configuration from environment variables.
    //    This must happen before init_tracer so the OTEL_* vars are visible.
    // -------------------------------------------------------------------------
    // Initialize advanced tracing
    init_tracing();

    // Initialise Prometheus metrics registry
    let _ = metrics::registry();

    dotenv().ok();

    let app_config = AppConfig::from_env().map_err(|e| {
        // We cannot use tracing here — the subscriber is not initialised yet.
        eprintln!("❌ Failed to load application configuration: {}", e);
        anyhow::anyhow!("Configuration error: {}", e)
    })?;

    app_config.validate().map_err(|e| {
        eprintln!("❌ Configuration validation failed: {}", e);
        anyhow::anyhow!("Configuration validation error: {}", e)
    })?;

    // Production-grade startup validation — enforces TLS, secret hygiene,
    // and environment-specific rules. Fatal in staging/production.
    if let Err(e) = config_validation::validate_production_config() {
        let app_env = std::env::var("APP_ENV").unwrap_or_else(|_| "development".into());
        if app_env != "development" {
            eprintln!("❌ {}", e);
            std::process::exit(1);
        } else {
            eprintln!("⚠️  Config warnings (non-fatal in development):\n{}", e);
        }
    }

    // -------------------------------------------------------------------------
    // 2. Initialise OpenTelemetry tracer provider.   (Issue #104)
    //
    //    init_tracer() must be called BEFORE any tracing::* macros fire so
    //    the global subscriber is registered and all spans are exported.
    //    It reads four fields from TelemetryConfig:
    //      • service_name  → OTEL_SERVICE_NAME
    //      • environment   → APP_ENV
    //      • sampling_rate → OTEL_SAMPLING_RATE
    //      • otlp_endpoint → OTEL_EXPORTER_OTLP_ENDPOINT
    // -------------------------------------------------------------------------
    init_tracer(&app_config.telemetry).map_err(|e| {
        eprintln!("❌ Failed to initialise OpenTelemetry tracer: {}", e);
        anyhow::anyhow!("Tracer initialisation error: {}", e)
    })?;

    // From this point all tracing::* calls produce structured JSON logs with
    // trace_id / span_id fields and export spans to the OTLP backend.

    let skip_externals = std::env::var("SKIP_EXTERNALS")
        .unwrap_or_else(|_| "false".to_string())
        .to_lowercase()
        == "true";

    info!(
        version = env!("CARGO_PKG_VERSION"),
        environment = %app_config.telemetry.environment,
        service = %app_config.telemetry.service_name,
        sampling_rate = app_config.telemetry.sampling_rate,
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
    let warming_state = WarmingState::new();
    let health_checker =
        HealthChecker::new(db_pool.clone(), redis_cache.clone(), stellar_client.clone())
            .with_warming_state(warming_state.clone());
        HealthChecker::new(db_pool.clone(), redis_cache.clone(), stellar_client.clone());

    // Spawn background task to update DB pool connection gauge every 15 seconds
    if let Some(pool) = db_pool.clone() {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(15));
            loop {
                ticker.tick().await;
                let stats = database::get_pool_stats(&pool);
                metrics::database::connections_active()
                    .with_label_values(&["primary"])
                    .set((stats.size - stats.num_idle) as f64);
            }
        });
    }


    // Initialize notification service
    let notification_service = std::sync::Arc::new(services::notification::NotificationService::new());

    // ── Audit logging system (Issue #183) ─────────────────────────────────────
    let audit_writer = if let (Some(ref pool), Some(ref redis_pool)) = (&db_pool, &redis_cache) {
        let audit_repo = std::sync::Arc::new(audit::repository::AuditLogRepository::new(pool.clone()));
        let audit_streamer = std::sync::Arc::new(audit::streaming::AuditStreamer::new(redis_pool.pool.clone()));
        let buffer_size: usize = std::env::var("AUDIT_WRITER_BUFFER_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4096);
        let (writer, rx) = audit::writer::AuditWriter::new(
            audit_repo.clone(),
            audit_streamer.clone(),
            Some(buffer_size),
        );
        let writer = std::sync::Arc::new(writer);
        tokio::spawn(audit::writer::run_writer_task(audit_repo, audit_streamer, rx));
        info!("✅ Audit logging writer started (buffer={})", buffer_size);
        Some(writer)
    } else {
        info!("⏭️  Skipping audit writer (no database/redis)");
        None
    };

    // Mint authorization audit service (chain-verified log trail)
    let mint_audit_service: Option<std::sync::Arc<audit::mint_authorization::MintAuthorizationService>> =
        if let Some(pool) = db_pool.clone() {
            let repo = audit::mint_authorization::MintAuthorizationRepository::new(pool.clone());
            Some(std::sync::Arc::new(audit::mint_authorization::MintAuthorizationService::new(repo)))
        } else {
            None
        };

    // --- Cache warming (must complete before traffic is accepted) ---
    if let (Some(ref pool), Some(ref redis)) = (&db_pool, &redis_cache) {
        let registry = prometheus::default_registry();
        let ml_cache = cache::build_multi_level_cache(redis.clone(), registry);
        let rate_repo = database::exchange_rate_repository::ExchangeRateRepository::new(pool.clone());
        let fee_repo = database::fee_structure_repository::FeeStructureRepository::new(pool.clone());
        let ws = warming_state.clone();
        let l1 = ml_cache.l1.clone();
        let redis_clone = redis.clone();
        tokio::spawn(async move {
            warm_caches(&l1, &redis_clone, &rate_repo, &fee_repo, &ws).await;
        });
    } else {
        // No DB or Redis — mark ready immediately so health check passes.
        warming_state.mark_ready();
    }

    // Initialize payment provider factory
    let provider_factory = if db_pool.is_some() {
        info!("💳 Initializing payment provider factory...");
        let factory = std::sync::Arc::new(PaymentProviderFactory::from_env().unwrap_or_else(|e| {
            error!("Failed to initialize payment provider factory: {}", e);
            panic!("Cannot start without payment providers");
        }));
        info!("✅ Payment provider factory initialized");
        Some(factory)
    } else {
        None
    };

    let (worker_shutdown_tx, worker_shutdown_rx) = watch::channel(false);
    
    // Start Transaction Monitor Worker
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
                mint_audit_service.clone(),
            );
            monitor_handle = Some(tokio::spawn(worker.run(worker_shutdown_rx.clone())));
        } else {
            info!(
                "Skipping Stellar transaction monitor worker (missing db pool or stellar client)"
            );
        }
    } else {
        info!("Stellar transaction monitor worker disabled (TX_MONITOR_ENABLED=false)");
    }

    // Start Offramp Processor Worker
    let offramp_enabled = std::env::var("OFFRAMP_PROCESSOR_ENABLED")
        .unwrap_or_else(|_| "true".to_string())
        .to_lowercase() != "false";
    let mut offramp_handle = None;
    if offramp_enabled {
        if let (Some(pool), Some(client), Some(factory)) = (db_pool.clone(), stellar_client.clone(), provider_factory.clone()) {
            let config = workers::offramp_processor::OfframpProcessorConfig::from_env();
            if let Err(e) = config.validate() {
                error!(error = %e, "Invalid offramp processor configuration, skipping worker");
            } else {
                info!(
                    poll_interval_secs = config.poll_interval.as_secs(),
                    batch_size = config.batch_size,
                    "Starting offramp processor worker"
                );
                let worker = workers::offramp_processor::OfframpProcessorWorker::new(
                    pool,
                    client,
                    factory,
                    notification_service.clone(),
                    config,
                );
                offramp_handle = Some(tokio::spawn(worker.run(worker_shutdown_rx.clone())));
            }
        } else {
            info!("Skipping offramp processor worker (missing db pool, stellar client, or provider factory)");
        }
    } else {
        info!("Offramp processor worker disabled (OFFRAMP_PROCESSOR_ENABLED=false)");
    }

    // Start Stellar Confirmation Polling Worker
    let stellar_confirm_enabled = std::env::var("STELLAR_CONFIRM_WORKER_ENABLED")
        .unwrap_or_else(|_| "true".to_string())
        .to_lowercase()
        != "false";
    if stellar_confirm_enabled {
        if let (Some(pool), Some(client)) = (db_pool.clone(), stellar_client.clone()) {
            let confirm_config =
                workers::stellar_confirmation_worker::StellarConfirmationConfig::from_env();
            let registry = prometheus::default_registry().clone();
            match workers::stellar_confirmation_worker::WorkerMetrics::new(&registry) {
                Ok(metrics) => {
                    info!(
                        poll_interval_secs = confirm_config.poll_interval.as_secs(),
                        confirmation_threshold = confirm_config.confirmation_threshold,
                        stale_timeout_secs = confirm_config.stale_timeout.as_secs(),
                        "Starting Stellar confirmation polling worker"
                    );
                    let worker = workers::stellar_confirmation_worker::StellarConfirmationWorker::new(
                        pool,
                        client,
                        confirm_config,
                        std::sync::Arc::new(metrics),
                    );
                    tokio::spawn(worker.run(worker_shutdown_rx.clone()));
                }
                Err(e) => {
                    error!(error = %e, "Failed to register Prometheus metrics for Stellar confirmation worker — skipping");
                }
            }
        } else {
            info!("Skipping Stellar confirmation worker (missing db pool or stellar client)");
        }
    } else {
        info!("Stellar confirmation worker disabled (STELLAR_CONFIRM_WORKER_ENABLED=false)");
    // Start Onramp Processor Worker
    let onramp_enabled = std::env::var("ONRAMP_PROCESSOR_ENABLED")
        .unwrap_or_else(|_| "true".to_string())
        .to_lowercase()
        != "false";
    let mut onramp_handle = None;
    if onramp_enabled {
        if let (Some(pool), Some(client), Some(factory)) =
            (db_pool.clone(), stellar_client.clone(), provider_factory.clone())
        {
            let config = workers::onramp_processor::OnrampProcessorConfig::from_env();
            if config.system_wallet_address.is_empty() || config.system_wallet_secret.is_empty() {
                error!("SYSTEM_WALLET_ADDRESS or SYSTEM_WALLET_SECRET not set — skipping onramp processor");
            } else {
                info!(
                    poll_interval_secs = config.poll_interval_secs,
                    pending_timeout_mins = config.pending_timeout_mins,
                    stellar_max_retries = config.stellar_max_retries,
                    "Starting onramp processor worker"
                );
                let processor = workers::onramp_processor::OnrampProcessor::new(
                    pool,
                    client,
                    std::sync::Arc::new(factory),
                    config,
                    mint_audit_service.clone(),
                );
                onramp_handle = Some(tokio::spawn(async move {
                    if let Err(e) = processor.run(worker_shutdown_rx.clone()).await {
                        error!(error = %e, "Onramp processor exited with error");
                    }
                }));
                info!("✅ Onramp processor worker started");
            }
        } else {
            info!("Skipping onramp processor worker (missing db pool, stellar client, or provider factory)");
        }
    } else {
        info!("Onramp processor worker disabled (ONRAMP_PROCESSOR_ENABLED=false)");
    // Start Bill Processor Worker
    let bill_processor_enabled = std::env::var("BILL_PROCESSOR_ENABLED")
        .unwrap_or_else(|_| "true".to_string())
        .to_lowercase() != "false";
    let mut bill_processor_handle = None;
    if bill_processor_enabled {
        if let (Some(pool), Some(client)) = (db_pool.clone(), stellar_client.clone()) {
            match workers::bill_processor::providers::BillProviderFactory::from_env() {
                Ok(bill_provider_factory) => {
                    let config = workers::bill_processor::worker::BillProcessorConfig::from_env();
                    info!(
                        poll_interval_secs = config.poll_interval.as_secs(),
                        "Starting bill processor worker"
                    );
                    let worker = workers::bill_processor::worker::BillProcessorWorker::new(
                        pool,
                        client,
                        Arc::new(bill_provider_factory),
                        notification_service.clone(),
                        config,
                    );
                    bill_processor_handle = Some(tokio::spawn(worker.run(worker_shutdown_rx.clone())));
                }
                Err(e) => {
                    error!(error = %e, "Failed to create bill provider factory, skipping worker");
                }
            }
        } else {
            info!("Skipping bill processor worker (missing db pool or stellar client)");
        }
    } else {
        info!("Bill processor worker disabled (BILL_PROCESSOR_ENABLED=false)");
    }


    // Start Payment Poller Worker
    let poller_enabled = std::env::var("PAYMENT_POLLER_ENABLED")
        .unwrap_or_else(|_| "true".to_string())
        .to_lowercase()
        != "false";
    if poller_enabled {
        if let (Some(pool), Some(factory)) = (db_pool.clone(), provider_factory.clone()) {
            let poller_config = workers::payment_poller::PaymentPollerConfig::from_env();
            info!(
                interval_secs = poller_config.poll_interval.as_secs(),
                max_retries = poller_config.max_retries,
                "Starting payment poller worker"
            );
            let tx_repo = std::sync::Arc::new(
                database::transaction_repository::TransactionRepository::new(pool.clone()),
            );
            let mut poller_providers = Vec::new();
            for provider_name in factory.list_available_providers() {
                if let Ok(p) = factory.get_provider(provider_name) {
                    poller_providers.push(
                        std::sync::Arc::from(p)
                            as std::sync::Arc<dyn payments::provider::PaymentProvider>,
                    );
                }
            }
            let poller_orchestrator = std::sync::Arc::new(
                services::payment_orchestrator::PaymentOrchestrator::new(
                    poller_providers,
                    tx_repo,
                    services::payment_orchestrator::OrchestratorConfig::default(),
                ),
            );
            let poller = workers::payment_poller::PaymentPollerWorker::new(
                pool,
                factory,
                poller_orchestrator,
                poller_config,
            );
            tokio::spawn(poller.run(worker_shutdown_rx.clone()));
            info!("✅ Payment poller worker started");
        } else {
            info!("⏭️  Skipping payment poller worker (missing db pool or provider factory)");
        }
    } else {
        info!("Payment poller worker disabled (PAYMENT_POLLER_ENABLED=false)");
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

        let mut exchange_rate_service = services::exchange_rate::ExchangeRateService::new(
            rate_repo,
            services::exchange_rate::ExchangeRateServiceConfig::default(),
        )
        .with_cache(cache.clone())
        .add_provider(std::sync::Arc::new(
            services::rate_providers::FixedRateProvider::new(),
        ));

        if let Ok(api_url) = std::env::var("EXTERNAL_RATE_API_URL") {
            let api_url = api_url.trim().to_string();
            if !api_url.is_empty() {
                let api_key = std::env::var("EXTERNAL_RATE_API_KEY")
                    .ok()
                    .and_then(|k| {
                        let trimmed = k.trim().to_string();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed)
                        }
                    });
                let timeout_secs = std::env::var("EXTERNAL_RATE_TIMEOUT_SECONDS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(10);

                let external_provider =
                    services::rate_providers::ExternalApiProvider::new(api_url.clone(), api_key)
                        .with_timeout(timeout_secs);

                exchange_rate_service =
                    exchange_rate_service.add_provider(std::sync::Arc::new(external_provider));

                info!(
                    external_rate_api_url = %api_url,
                    timeout_seconds = timeout_secs,
                    "External rate provider enabled"
                );
            }
        }

        let exchange_rate_service =
            std::sync::Arc::new(exchange_rate_service.with_fee_service(fee_service.clone()));

        let quote_service = std::sync::Arc::new(services::onramp_quote::OnrampQuoteService::new(
            exchange_rate_service,
            fee_service,
            client.clone(),
            cache.clone(),
            cngn_issuer,
        ));

        // Setup onramp status service
        let transaction_repo = std::sync::Arc::new(
            database::transaction_repository::TransactionRepository::new(pool.clone()),
        );
        let payment_factory =
            std::sync::Arc::new(PaymentProviderFactory::from_env().unwrap_or_else(|e| {
                error!("Failed to initialize payment provider factory for onramp status: {}", e);
                panic!("Cannot start without payment providers");
            }));
        
        let stellar_client_arc = std::sync::Arc::new(client);

        let status_service = std::sync::Arc::new(api::onramp::OnrampStatusService::new(
            transaction_repo.clone(),
            std::sync::Arc::new(cache.clone()),
            stellar_client_arc.clone(),
            payment_factory.clone(),
        ));

        let cngn_issuer_for_initiate = std::env::var("CNGN_ISSUER_ADDRESS")
            .or_else(|_| std::env::var("CNGN_ISSUER_MAINNET"))
            .unwrap_or_else(|_| "GXXXXDEFAULTISSUERXXXX".to_string());

        // Build orchestrator for initiate endpoint (#20)
        let mut onramp_providers = Vec::new();
        for provider_name in payment_factory.list_available_providers() {
            if let Ok(p) = payment_factory.get_provider(provider_name) {
                onramp_providers.push(
                    std::sync::Arc::from(p) as std::sync::Arc<dyn payments::provider::PaymentProvider>,
                );
            }
        }
        let onramp_orchestrator = std::sync::Arc::new(
            services::payment_orchestrator::PaymentOrchestrator::new(
                onramp_providers,
                transaction_repo.clone(),
                services::payment_orchestrator::OrchestratorConfig::from_env(),
            ),
        );

        let initiate_state = std::sync::Arc::new(api::onramp::OnrampInitiateState {
            transaction_repo,
            cache: std::sync::Arc::new(cache.clone()),
            stellar_client: stellar_client_arc,
            orchestrator: onramp_orchestrator,
            cngn_issuer: cngn_issuer_for_initiate,
        });

        let onramp_integrity_state = crate::middleware::request_integrity::RequestIntegrityState {
            endpoint: crate::middleware::request_integrity::IntegrityEndpoint::OnrampInitiate,
            db: db_pool.clone().map(std::sync::Arc::new),
            cache: Some(std::sync::Arc::new(cache.clone())),
        };

        Router::new()
            .route("/api/onramp/quote", post(create_onramp_quote))
            .with_state(quote_service)
            .route("/api/onramp/status/:tx_id", get(api::onramp::get_onramp_status))
            .with_state(status_service)
            .route(
                "/api/onramp/initiate",
                post(api::onramp::initiate_onramp).route_layer(axum::middleware::from_fn_with_state(
                    onramp_integrity_state,
                    crate::middleware::request_integrity::request_integrity_middleware,
                )),
            )
            .with_state(initiate_state)
    } else {
        Router::new()
    };

    // Setup wallet routes with balance service
    let wallet_routes = if let (Some(client), Some(cache)) = (stellar_client.clone(), redis_cache.clone()) {
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
    
    // Setup rates API routes with exchange rate service
    let rates_routes = if let Some(pool) = db_pool.clone() {
        use database::exchange_rate_repository::ExchangeRateRepository;
        use services::exchange_rate::{ExchangeRateService, ExchangeRateServiceConfig};
        
        let repository = ExchangeRateRepository::new(pool.clone());
        let config = ExchangeRateServiceConfig::default();
        let mut exchange_rate_service = ExchangeRateService::new(repository, config)
            .add_provider(std::sync::Arc::new(
                services::rate_providers::FixedRateProvider::new(),
            ));
        
        // Add cache to exchange rate service if available
        if let Some(ref cache) = redis_cache {
            exchange_rate_service = exchange_rate_service.with_cache(cache.clone());
        }

        if let Ok(api_url) = std::env::var("EXTERNAL_RATE_API_URL") {
            let api_url = api_url.trim().to_string();
            if !api_url.is_empty() {
                let api_key = std::env::var("EXTERNAL_RATE_API_KEY")
                    .ok()
                    .and_then(|k| {
                        let trimmed = k.trim().to_string();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed)
                        }
                    });
                let timeout_secs = std::env::var("EXTERNAL_RATE_TIMEOUT_SECONDS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(10);

                let external_provider =
                    services::rate_providers::ExternalApiProvider::new(api_url.clone(), api_key)
                        .with_timeout(timeout_secs);

                exchange_rate_service =
                    exchange_rate_service.add_provider(std::sync::Arc::new(external_provider));

                info!(
                    external_rate_api_url = %api_url,
                    timeout_seconds = timeout_secs,
                    "External rate provider enabled for /api/rates"
                );
            }
        }
        
        let rates_state = api::rates::RatesState {
            exchange_rate_service: std::sync::Arc::new(exchange_rate_service),
            cache: redis_cache.clone().map(std::sync::Arc::new),
        };
        
        Router::new()
            .route("/api/rates", get(api::rates::get_rates).options(api::rates::options_rates))
            .with_state(rates_state)
    } else {
        info!("⏭️  Skipping rates routes (no database)");
        Router::new()
    };

    // Setup offramp routes (withdrawal initiation)
    let offramp_routes = if let (Some(pool), Some(cache)) = (db_pool.clone(), redis_cache.clone()) {
        let system_wallet_address = std::env::var("SYSTEM_WALLET_ADDRESS")
            .or_else(|_| std::env::var("SYSTEM_WALLET_MAINNET"))
            .unwrap_or_else(|_| "GSYSTEMWALLETXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string());

        let cngn_issuer_address = std::env::var("CNGN_ISSUER_ADDRESS")
            .or_else(|_| std::env::var("CNGN_ISSUER_MAINNET"))
            .unwrap_or_else(|_| "GCNGNISSUERXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string());

        let payment_factory = std::sync::Arc::new(PaymentProviderFactory::from_env().unwrap_or_else(|e| {
            error!("Failed to initialize payment provider factory for offramp: {}", e);
            panic!("Cannot start without payment providers");
        }));

        // Initialize bank verification service
        let bank_verification_config = services::bank_verification::BankVerificationConfig {
            timeout_secs: std::env::var("BANK_VERIFICATION_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(30),
            max_retries: std::env::var("BANK_VERIFICATION_MAX_RETRIES")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(2),
            name_match_tolerance: std::env::var("BANK_VERIFICATION_NAME_MATCH_TOLERANCE")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.7),
        };

        let bank_verification_service = std::sync::Arc::new(
            services::bank_verification::BankVerificationService::new(payment_factory.clone(), bank_verification_config)
        );

        let offramp_state = api::offramp::OfframpState {
            db_pool: std::sync::Arc::new(pool),
            redis_cache: std::sync::Arc::new(cache),
            payment_provider_factory: payment_factory,
            bank_verification_service,
            system_wallet_address,
            cngn_issuer_address,
        };

        let offramp_integrity_state = crate::middleware::request_integrity::RequestIntegrityState {
            endpoint: crate::middleware::request_integrity::IntegrityEndpoint::OfframpInitiate,
            db: Some(offramp_state.db_pool.clone()),
            cache: Some(offramp_state.redis_cache.clone()),
        };

        Router::new()
            .route(
                "/api/offramp/initiate",
                post(api::offramp::initiate_withdrawal).route_layer(axum::middleware::from_fn_with_state(
                    offramp_integrity_state,
                    crate::middleware::request_integrity::request_integrity_middleware,
                )),
            )
            .with_state(std::sync::Arc::new(offramp_state))
    } else {
        info!("⏭️  Skipping offramp routes (missing database or cache)");
        Router::new()
    };
    
    // Setup fees API routes with fee calculation service
    let fees_routes = if let Some(pool) = db_pool.clone() {
        use services::fee_calculation::FeeCalculationService;
        
        let fee_service = std::sync::Arc::new(FeeCalculationService::new(pool.clone()));
        
        let fees_state = api::fees::FeesState {
            fee_service,
            cache: redis_cache.clone(),
        };
        
        Router::new()
            .route("/api/fees", get(api::fees::get_fees))
            .with_state(fees_state)
    } else {
        info!("⏭️  Skipping fees routes (no database)");
        Router::new()
    };

    // Setup transaction history routes
    let history_routes = if let Some(pool) = db_pool.clone() {
        let history_state = std::sync::Arc::new(api::transaction_history::TransactionHistoryState {
            pool: std::sync::Arc::new(pool),
            cache: redis_cache.clone().map(std::sync::Arc::new),
        });
        Router::new()
            .route("/api/transactions", get(api::transaction_history::get_transaction_history))
            .route("/api/transactions/export", get(api::transaction_history::export_transaction_history))
            .with_state(history_state)
    } else {
        info!("⏭️  Skipping transaction history routes (no database)");
        Router::new()
    };

    // Setup auth routes
    let auth_routes = if let Some(cache) = redis_cache.clone() {
        let auth_state = api::auth::AuthState {
            redis_cache: std::sync::Arc::new(cache),
        };
        Router::new()
            .route("/api/auth/challenge", post(api::auth::generate_challenge))
            .route("/api/auth/verify", post(api::auth::verify_signature))
            .with_state(std::sync::Arc::new(auth_state))
    } else {
        info!("⏭️  Skipping auth routes (missing cache)");
        Router::new()
    };
    
    // Setup auth routes
    let auth_routes = {
        let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| {
            tracing::warn!("JWT_SECRET not set – auth endpoints will be unavailable");
            String::new()
        });
        if jwt_secret.len() >= 32 {
            let auth_state = std::sync::Arc::new(auth::AuthHandlerState {
                jwt_secret,
                redis_cache: redis_cache.clone(),
            });
            info!("🔐 JWT auth routes enabled");
            auth::auth_router(auth_state)
        } else {
            info!("⏭️  Skipping auth routes (JWT_SECRET not set or too short)");
            Router::new()
        }
    };

    // ── Recurring payment routes (Issue #122) ────────────────────────────────
    let recurring_routes = if let Some(pool) = db_pool.clone() {
        let failure_threshold = std::env::var("RECURRING_FAILURE_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);
        let recurring_state = std::sync::Arc::new(api::recurring::RecurringState {
            repo: std::sync::Arc::new(
                database::recurring_payment_repository::RecurringPaymentRepository::new(pool.clone()),
            ),
            default_failure_threshold: failure_threshold,
        });
        Router::new()
            .route("/api/recurring", post(api::recurring::create_schedule))
            .route("/api/recurring", get(api::recurring::list_schedules))
            .route("/api/recurring/{id}", get(api::recurring::get_schedule))
            .route("/api/recurring/{id}", patch(api::recurring::update_schedule))
            .route("/api/recurring/{id}", delete(api::recurring::cancel_schedule))
            .with_state(recurring_state)
    } else {
        info!("Skipping recurring routes (no database)");
        Router::new()
    };

    // Start Recurring Payment Worker (Issue #122)
    let recurring_worker_enabled = std::env::var("RECURRING_WORKER_ENABLED")
        .unwrap_or_else(|_| "true".to_string())
        .to_lowercase()
        != "false";
    if recurring_worker_enabled {
        if let Some(pool) = db_pool.clone() {
            let worker_config = workers::recurring_payment_worker::RecurringWorkerConfig::from_env();
            info!(
                poll_interval_secs = worker_config.poll_interval.as_secs(),
                batch_size = worker_config.batch_size,
                "Starting recurring payment worker"
            );
            let repo = std::sync::Arc::new(
                database::recurring_payment_repository::RecurringPaymentRepository::new(pool),
            );
            let worker = workers::recurring_payment_worker::RecurringPaymentWorker::new(
                repo,
                worker_config,
            );
            tokio::spawn(worker.run(worker_shutdown_rx.clone()));
            info!("✅ Recurring payment worker started");
        } else {
            info!("Skipping recurring payment worker (no database)");
        }
    } else {
        info!("Recurring payment worker disabled (RECURRING_WORKER_ENABLED=false)");
    }

    // ── IP Detection Worker (Issue #166) ─────────────────────────────────────
    let ip_detection_worker_enabled = std::env::var("IP_DETECTION_WORKER_ENABLED")
        .unwrap_or_else(|_| "true".to_string())
        .to_lowercase()
        != "false";
    if ip_detection_worker_enabled {
        if let (Some(pool), Some(cache)) = (db_pool.clone(), redis_cache.clone()) {
            let detection_service = std::sync::Arc::new(
                crate::services::ip_detection::IpDetectionService::new(
                    database::ip_reputation_repository::IpReputationRepository::new(pool),
                    std::sync::Arc::new(cache),
                    Default::default(),
                )
            );

            // Bootstrap blocked IPs cache on startup
            if let Err(e) = detection_service.bootstrap_blocked_ips_cache().await {
                error!(error = %e, "Failed to bootstrap blocked IPs cache");
            }

            let worker_config = workers::ip_detection_worker::IpDetectionWorkerConfig::from_env();
            let worker = workers::ip_detection_worker::IpDetectionWorker::new(
                database::ip_reputation_repository::IpReputationRepository::new(db_pool.clone().unwrap()),
                detection_service,
                worker_config,
            );
            tokio::spawn(worker.run(worker_shutdown_rx.clone()));
            info!("✅ IP detection worker started");
        } else {
            info!("Skipping IP detection worker (missing database or cache)");
        }
    } else {
        info!("IP detection worker disabled (IP_DETECTION_WORKER_ENABLED=false)");
    }

    // ── Batch transaction routes (Issue #125) ────────────────────────────────
    let batch_routes = if let Some(pool) = db_pool.clone() {
        let batch_state = api::batch::BatchState::new(std::sync::Arc::new(pool));
        let batch_cngn_integrity_state = crate::middleware::request_integrity::RequestIntegrityState {
            endpoint: crate::middleware::request_integrity::IntegrityEndpoint::BatchCngnTransfer,
            db: Some(batch_state.db.clone()),
            cache: redis_cache.clone().map(std::sync::Arc::new),
        };
        let batch_fiat_integrity_state = crate::middleware::request_integrity::RequestIntegrityState {
            endpoint: crate::middleware::request_integrity::IntegrityEndpoint::BatchFiatPayout,
            db: Some(batch_state.db.clone()),
            cache: redis_cache.clone().map(std::sync::Arc::new),
        };
        Router::new()
            .route(
                "/api/batch/cngn-transfer",
                post(api::batch::create_cngn_transfer_batch).route_layer(axum::middleware::from_fn_with_state(
                    batch_cngn_integrity_state,
                    crate::middleware::request_integrity::request_integrity_middleware,
                )),
            )
            .route(
                "/api/batch/fiat-payout",
                post(api::batch::create_fiat_payout_batch).route_layer(axum::middleware::from_fn_with_state(
                    batch_fiat_integrity_state,
                    crate::middleware::request_integrity::request_integrity_middleware,
                )),
            )
            .route("/api/batch/{batch_id}",    get(api::batch::get_batch_status))
            .with_state(batch_state)
    } else {
        info!("Skipping batch routes (no database)");
        Router::new()
    };

    // ── Admin scope management routes (Issue #132) ───────────────────────────
    let admin_routes = if let Some(pool) = db_pool.clone() {
        let scopes_state = api::admin::scopes::ScopesState {
            db: std::sync::Arc::new(pool.clone()),
        };
        let keys_state = api::admin::keys::AdminKeysState {
            db: std::sync::Arc::new(pool.clone()),
        };
        let ip_reputation_state = api::admin::ip_reputation::IpReputationState {
            repo: database::ip_reputation_repository::IpReputationRepository::new(pool.clone()),
        };
        Router::new()

        // ── Revocation & Blacklist routes (Issue #138) ────────────────────────
        let revocation_state = if let Some(ref redis) = redis_cache {
            let svc = std::sync::Arc::new(services::revocation::RevocationService::new(
                std::sync::Arc::new(pool.clone()),
                std::sync::Arc::new(redis.clone()),
                notification_service.clone(),
            ));
            let svc_clone = svc.clone();
            tokio::spawn(async move {
                if let Err(e) = svc_clone.bootstrap_redis_blacklist().await {
                    tracing::error!(error = %e, "Redis blacklist bootstrap failed");
                }
            });
            Some(api::admin::revocation::RevocationState { service: svc })
        } else {
            info!("Skipping revocation service (no Redis)");
            None
        };

        let mut router = Router::new()
            .route("/api/admin/scopes", get(api::admin::scopes::list_scopes))
            .route(
                "/api/admin/consumers/{consumer_id}/keys/{key_id}/scopes",
                get(api::admin::scopes::get_key_scopes)
                    .patch(api::admin::scopes::update_key_scopes),
            )
            .with_state(scopes_state)
            .merge(
                Router::new()
                    // Issue #131 — API key issuance
                    .route(
                        "/api/admin/consumers/{consumer_id}/keys",
                        post(api::admin::keys::issue_key)
                            .get(api::admin::keys::list_keys),
                    )
                    .route(
                        "/api/admin/consumers/{consumer_id}/keys/{key_id}",
                        delete(api::admin::keys::revoke_key),
                    )
                    .with_state(keys_state),
            )
            .merge(
                Router::new()
                    // Issue #166 — IP reputation management
                    .route(
                        "/api/admin/ip-reputation",
                        get(api::admin::ip_reputation::list_flagged_ips),
                    )
                    .route(
                        "/api/admin/ip-reputation/{ip}",
                        get(api::admin::ip_reputation::get_ip_reputation),
                    )
                    .route(
                        "/api/admin/ip-reputation/{ip}/block",
                        post(api::admin::ip_reputation::block_ip),
                    )
                    .route(
                        "/api/admin/ip-reputation/{ip}/unblock",
                        post(api::admin::ip_reputation::unblock_ip),
                    )
                    .route(
                        "/api/admin/ip-reputation/{ip}/whitelist",
                        post(api::admin::ip_reputation::whitelist_ip),
                    )
                    .with_state(ip_reputation_state),
            )
    } else {
        info!("Skipping admin routes (no database)");
        Router::new()
    };

    // ── mTLS certificate lifecycle — Issue #204 ───────────────────────────────
    // Provision the intermediate CA and start the lifecycle worker.
    // The admin routes are always available (they operate on the in-memory store).
    let mtls_admin_routes = {
        use mtls::{
            MtlsConfig, IntermediateCa, CertificateStore, CertificateProvisioner,
            RevocationService, CertLifecycleWorker,
        };        use mtls::revocation::RevocationList;
        use mtls::admin::{MtlsAdminState, mtls_admin_routes};

        let mtls_config = MtlsConfig::from_env().unwrap_or_else(|e| {
            tracing::warn!("mTLS config error (using defaults): {}", e);
            MtlsConfig::from_env().unwrap_or_else(|_| MtlsConfig {
                service_name: "aframp-backend".to_string(),
                environment: std::env::var("APP_ENV").unwrap_or_else(|_| "development".to_string()),
                leaf_cert_validity: std::time::Duration::from_secs(90 * 86400),
                intermediate_cert_validity: std::time::Duration::from_secs(730 * 86400),
                rotation_threshold_days: 14,
                alert_threshold_days: 7,
                intermediate_ca_cert_pem: String::new(),
                intermediate_ca_key_pem: String::new(),
                root_ca_cert_pem: String::new(),
                ca_distribution_url: String::new(),
                enforce_mtls: false,
            })
        });

        // Register mTLS Prometheus metrics
        mtls::metrics::register(prometheus::default_registry());

        let cert_store = CertificateStore::new();
        let crl = RevocationList::new();
        let revocation_svc = std::sync::Arc::new(RevocationService::new(crl, cert_store.clone()));

        // Only start the CA and provisioner if the intermediate CA PEM is configured.
        let provisioner = if !mtls_config.intermediate_ca_cert_pem.is_empty() {
            match IntermediateCa::from_pem(&mtls_config) {
                Ok(ca) => {
                    let ca = std::sync::Arc::new(ca);
                    let p = std::sync::Arc::new(CertificateProvisioner::new(
                        ca,
                        cert_store.clone(),
                        revocation_svc.clone(),
                        mtls_config.clone(),
                    ));
                    // Provision all registered services at startup
                    for &svc in mtls::cert::REGISTERED_SERVICES {
                        match p.provision_at_startup(svc) {
                            Ok(cert) => info!(
                                service = svc,
                                serial = %cert.serial,
                                expires_at = %cert.expires_at,
                                "mTLS: startup certificate provisioned"
                            ),
                            Err(e) => tracing::warn!(service = svc, error = %e, "mTLS: startup provisioning failed"),
                        }
                    }
                    // Start the lifecycle worker (14-day rotation sweep)
                    let worker = CertLifecycleWorker::new(p.clone(), cert_store.clone(), mtls_config.clone());
                    tokio::spawn(worker.run(worker_shutdown_rx.clone()));
                    info!("✅ mTLS certificate lifecycle worker started");
                    p
                }
                Err(e) => {
                    tracing::warn!("mTLS: intermediate CA not loaded ({}); admin endpoints available but no auto-provisioning", e);
                    std::sync::Arc::new(CertificateProvisioner::without_ca(
                        cert_store.clone(),
                        revocation_svc.clone(),
                        mtls_config.clone(),
                    ))
                }
            }
        } else {
            info!("mTLS: MTLS_INTERMEDIATE_CA_CERT_PEM not set — certificate auto-provisioning disabled");
            std::sync::Arc::new(CertificateProvisioner::without_ca(
                cert_store.clone(),
                revocation_svc.clone(),
                mtls_config.clone(),
            ))
        };

        let mtls_state = std::sync::Arc::new(MtlsAdminState {
            store: cert_store,
            provisioner,
            revocation: revocation_svc,
        });

        mtls_admin_routes()
            .with_state(mtls_state)
            .route_layer(axum::middleware::from_fn(security_headers_middleware))
    };

    // ── DDoS protection state and admin routes ────────────────────────────────
    // ── Audit log query routes (Issue #183) ──────────────────────────────────
    let audit_routes = if let Some(ref pool) = db_pool {
        let audit_handler_state = std::sync::Arc::new(audit::handlers::AuditHandlerState {
            repo: std::sync::Arc::new(audit::repository::AuditLogRepository::new(pool.clone())),
        });
        Router::new()
            .route("/api/admin/audit/logs", get(audit::handlers::list_audit_logs))
            .route("/api/admin/audit/logs/export", get(audit::handlers::export_audit_logs))
            .route("/api/admin/audit/logs/verify", get(audit::handlers::verify_hash_chain))
            .route("/api/admin/audit/logs/:entry_id", get(audit::handlers::get_audit_log_entry))
            .with_state(audit_handler_state)
    } else {
        Router::new()
    };
    let (ddos_state, ddos_admin_routes) = if let Some(ref cache) = redis_cache {
        let ddos_config = ddos::config::DdosConfig::from_env();
        let state = std::sync::Arc::new(ddos::state::DdosState::new(ddos_config, cache.clone()));
        // Spawn CDN sync background task
        {
            let s = state.clone();
            let interval = state.config.cdn_sync_interval_secs;
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval));
                loop { ticker.tick().await; s.sync_cdn_blocklist().await; }
            });
        }
        let routes = ddos::admin::ddos_admin_router(state.clone());
        info!("✅ DDoS protection enabled");
        (Some(state), routes)
    } else {
        info!("⏭️  Skipping DDoS protection (no Redis cache)");
        (None, Router::new())
    };

    // ── Key rotation routes (Issue #137) ─────────────────────────────────────
    let key_rotation_routes = if let Some(pool) = db_pool.clone() {
        let rotation_state = api::key_rotation::KeyRotationState {
            db: std::sync::Arc::new(pool.clone()),
        };
        let rotation_service = services::key_rotation::KeyRotationService::new(pool.clone());
        let rotation_worker = workers::key_rotation_worker::KeyRotationWorker::new(rotation_service);
        tokio::spawn(rotation_worker.run(worker_shutdown_rx.clone()));
        info!("✅ Key rotation worker started");
        api::key_rotation::developer_rotation_router(rotation_state.clone())
            .merge(api::key_rotation::admin_rotation_router(rotation_state))
    } else {
        info!("Skipping key rotation routes (no database)");
    // ── Developer self-service key routes (Issue #131) ───────────────────────
    let developer_routes = if let Some(pool) = db_pool.clone() {
        let dev_state = api::developer::keys::DeveloperKeysState {
            db: std::sync::Arc::new(pool),
        };
        Router::new()
            .route("/api/developer/keys", post(api::developer::keys::issue_key))
            .route("/api/developer/keys", get(api::developer::keys::list_keys))
            .route("/api/developer/keys/{key_id}", delete(api::developer::keys::revoke_key))
            .with_state(dev_state)
    } else {
        info!("Skipping developer routes (no database)");
        Router::new()
    };

    // ── OpenAPI / Swagger UI (Issue #114) ────────────────────────────────────
    let openapi_routes = api::openapi::openapi_routes();

    // ── Pentest & Security Review Framework ──────────────────────────────────
    let pentest_routes = if let Some(pool) = db_pool.clone() {
        let repo = std::sync::Arc::new(pentest::PentestRepository::new(pool));
        let svc = std::sync::Arc::new(pentest::PentestService::new(repo));
        // Spawn safety backstop task — auto-deactivates expired pentest windows
        {
            let svc_clone = svc.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    ticker.tick().await;
                    let _ = svc_clone.run_safety_backstop().await;
                }
            });
        }
        // Spawn SLA breach alert task — fires every 15 minutes
        {
            let svc_clone = svc.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(900));
                loop {
                    ticker.tick().await;
                    let _ = svc_clone.check_sla_breaches().await;
                }
            });
        }
        info!("🔒 Pentest & security review framework routes enabled");
        pentest::pentest_routes(svc)
    } else {
        info!("⏭️  Skipping pentest routes (no database)");
        Router::new()
    };

    // Setup OAuth 2.0 routes
    let oauth_routes = if let (Some(pool), Some(cache)) = (db_pool.clone(), redis_cache.clone()) {
        match oauth::RsaKeyPair::from_env() {
            Ok(key_pair) => {
                let issuer = std::env::var("OAUTH_ISSUER")
                    .unwrap_or_else(|_| "https://api.aframp.com".to_string());
                let is_production = std::env::var("ENVIRONMENT")
                    .unwrap_or_default()
                    .to_lowercase() == "production";
                let oauth_state = std::sync::Arc::new(oauth::OAuthState {
                    db_pool: pool,
                    redis_cache: cache,
                    key_pair: std::sync::Arc::new(key_pair),
                    issuer,
                    is_production,
                });
                info!("🔑 OAuth 2.0 routes enabled (RS256)");
                oauth::oauth_router(oauth_state)
            }
            Err(e) => {
                tracing::warn!("⏭️  Skipping OAuth routes: {}", e);
                Router::new()
            }
        }
    } else {
        info!("⏭️  Skipping OAuth routes (missing database or cache)");
        Router::new()
    };
    let app = Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .route("/health/ready", get(readiness))
        .route("/health/live", get(liveness))
        .route("/metrics", get(metrics::handler::metrics_handler))
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
        .merge(offramp_routes)
        .merge(wallet_routes)
        .merge(rates_routes)
        .merge(fees_routes)
        .merge(webhook_routes)
        .merge(history_routes)
        .merge(auth_routes)
        .merge(batch_routes)
        .merge(admin_routes)
        .merge(key_rotation_routes)
        .merge(openapi_routes)
        .merge(recurring_routes)
    let app = Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .route("/health/ready", get(readiness))
        .route("/health/live", get(liveness))
        .route("/metrics", get(metrics::handler::metrics_handler))
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
        .merge(offramp_routes)
        .merge(wallet_routes)
        .merge(rates_routes)
        .merge(fees_routes)
        .merge(webhook_routes)
        .merge(history_routes)
        .merge(auth_routes)
        .merge(batch_routes)
        .merge(admin_routes)
        .merge(audit_routes)
        .merge(key_rotation_routes)
        .merge(openapi_routes)
        .merge(recurring_routes)
        .merge(developer_routes)
        .merge(oauth_routes)
        .merge(history_routes)
        .merge(ddos_admin_routes)
        .merge(pentest_routes)
        .merge(developer_portal::routes::register_developer_portal_routes(Router::new(), db_pool.clone()))
        .merge(Router::new().nest("/api/admin/security", mtls_admin_routes))
        .with_state(AppState {
            db_pool,
            redis_cache,
            stellar_client,
            health_checker,
            warming_state: Some(warming_state),
        });

    // Apply middleware conditionally based on available services
    let app = if let (Some(db_pool), Some(redis_cache)) = (db_pool.clone(), redis_cache.clone()) {
        let ip_blocking_state = crate::middleware::ip_blocking::IpBlockingState {
            detection_service: std::sync::Arc::new(
                crate::services::ip_detection::IpDetectionService::new(
                    database::ip_reputation_repository::IpReputationRepository::new(db_pool),
                    std::sync::Arc::new(redis_cache.clone()),
                    Default::default(),
                )
            ),
        };

        app.layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(UuidRequestId))
                .layer(axum::middleware::from_fn(
                    crate::telemetry::middleware::tracing_middleware,
                ))
                .layer(axum::middleware::from_fn_with_state(
                    ip_blocking_state,
                    crate::middleware::ip_blocking::ip_blocking_middleware,
                ))
                .layer(axum::middleware::from_fn(metrics_middleware))
                .layer(axum::middleware::from_fn(request_logging_middleware))
                .layer(PropagateRequestIdLayer::x_request_id()),
        )
        .layer(
            // ---------------------------------------------------------------
            // Middleware stack — innermost layer runs first on the way in,
            // last on the way out.
            //
            // Order (outermost → innermost, i.e. the order added here):
            //   1. CORS middleware         — handles cross-origin requests
            //   2. Security headers        — adds security headers to responses
            //   3. SetRequestIdLayer       — assigns x-request-id UUID
            //   4. tracing_middleware      — extracts W3C traceparent, opens
            //                               root span per request (Issue #104)
            //   5. request_logging_middleware — structured access log line
            //   6. PropagateRequestIdLayer — copies x-request-id to response
            //
            // The tracing middleware is inserted between SetRequestId and the
            // existing request_logging_middleware so:
            //   • The request ID is already set when the span is created.
            //   • The access log fires inside the span and therefore inherits
            //     trace_id / span_id in its JSON output.
            // ---------------------------------------------------------------
            ServiceBuilder::new()
                .layer(axum::middleware::from_fn_with_state(
                    CorsConfig::from_env(),
                    cors_middleware,
                ))
                .layer(axum::middleware::from_fn(security_headers_middleware))
                .layer(SetRequestIdLayer::x_request_id(UuidRequestId))
                .layer(axum::middleware::from_fn(
                    crate::telemetry::middleware::tracing_middleware,
                ))
                .layer(axum::middleware::from_fn(metrics_middleware))
                .layer(axum::middleware::from_fn(request_logging_middleware))
                .layer(PropagateRequestIdLayer::x_request_id()),
        )
    };

    let rate_limit_config = std::sync::Arc::new(crate::middleware::rate_limit::RateLimitConfig::load("rate_limits.yaml").unwrap_or_else(|e| {
        tracing::warn!("Failed to load rate_limits.yaml, using defaults: {}", e);
        crate::middleware::rate_limit::RateLimitConfig {
            endpoints: std::collections::HashMap::new(),
            default: crate::middleware::rate_limit::EndpointLimits {
                per_ip: Some(crate::middleware::rate_limit::LimitConfig { limit: 100, window: 60 }),
                per_wallet: None,
            }
        }
    }));

    let app = if let Some(cache) = redis_cache.clone() {

        let rate_limit_state = crate::middleware::rate_limit::RateLimitState {
            cache: std::sync::Arc::new(cache.clone()),
            config: rate_limit_config,
        };

        let replay_state = crate::middleware::replay_prevention::ReplayPreventionState {
            redis: std::sync::Arc::new(cache.pool.clone()),
            config: std::sync::Arc::new(crate::middleware::replay_prevention::ReplayConfig::from_env()),
        };

        app
            .layer(axum::middleware::from_fn_with_state(
                replay_state,
                crate::middleware::replay_prevention::replay_prevention_middleware,
            ))
            .layer(axum::middleware::from_fn_with_state(rate_limit_state, crate::middleware::rate_limit::rate_limit_middleware))
    } else {
        app
    };

    // Apply DDoS middleware if state was initialised
    let app = if let Some(ds) = ddos_state {
        app.layer(axum::middleware::from_fn_with_state(
            ds,
            crate::ddos::middleware::ddos_middleware,
        ))
    } else {
        app
    };


    info!("✅ Routes configured");

    // Inject audit writer as an Axum extension so the middleware can access it
    let app = if let Some(ref writer) = audit_writer {
        app.layer(axum::Extension(writer.clone()))
            .layer(axum::middleware::from_fn(audit::middleware::audit_middleware))
    } else {
        app
    };

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
    println!("║  GET  /api/rates                 - Exchange rates (public)  ║");
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
    if let Some(handle) = offramp_handle {
        if let Err(e) = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
            error!(error = %e, "Timed out waiting for offramp worker shutdown");
        }
    }

    info!("👋 Server shutdown complete");

    // -------------------------------------------------------------------------
    // Flush all buffered spans to the OTLP exporter before the process exits.
    // Must be the very last call so no spans are lost during shutdown.   (Issue #104)
    // -------------------------------------------------------------------------
    shutdown_tracer();

    Ok(())
}

// Application state
#[derive(Clone)]
struct AppState {
    db_pool: Option<sqlx::PgPool>,
    redis_cache: Option<RedisCache>,
    stellar_client: Option<StellarClient>,
    health_checker: HealthChecker,
    warming_state: Option<WarmingState>,
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
