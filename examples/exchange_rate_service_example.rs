//! Exchange Rate Service Usage Examples
//!
//! This file demonstrates how to use the Exchange Rate Service
//! for fetching rates, calculating conversions, and managing historical data.

#[cfg(feature = "database")]
use bigdecimal::BigDecimal;
#[cfg(feature = "database")]
use sqlx::PgPool;
#[cfg(feature = "database")]
use std::sync::Arc;
#[cfg(all(feature = "database", feature = "cache"))]
use Bitmesh_backend::cache::cache::RedisCache;
#[cfg(all(feature = "database", feature = "cache"))]
use Bitmesh_backend::cache::{init_cache_pool, CacheConfig};
#[cfg(feature = "database")]
use Bitmesh_backend::database::exchange_rate_repository::ExchangeRateRepository;
#[cfg(feature = "database")]
use Bitmesh_backend::database::fee_structure_repository::FeeStructureRepository;
#[cfg(feature = "database")]
use Bitmesh_backend::services::exchange_rate::{
    ConversionDirection, ConversionRequest, ExchangeRateService, ExchangeRateServiceConfig,
};
#[cfg(feature = "database")]
use Bitmesh_backend::services::fee_structure::FeeStructureService;
#[cfg(feature = "database")]
use Bitmesh_backend::services::rate_providers::FixedRateProvider;

#[cfg(all(feature = "database", feature = "cache"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    println!("=== Exchange Rate Service Examples ===\n");

    // Setup database connection
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://localhost/aframp".to_string());
    let pool = PgPool::connect(&database_url).await?;

    // Setup Redis cache
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());
    let cache_config = CacheConfig {
        redis_url,
        ..Default::default()
    };
    let cache_pool = init_cache_pool(cache_config).await?;
    let cache = RedisCache::new(cache_pool);

    // Create repositories
    let rate_repo = ExchangeRateRepository::with_cache(pool.clone(), cache.clone());
    let fee_repo = FeeStructureRepository::new(pool.clone());

    // Create services
    let fee_service = Arc::new(FeeStructureService::new(fee_repo));
    let rate_provider = Arc::new(FixedRateProvider::new());

    let exchange_service =
        ExchangeRateService::new(rate_repo, ExchangeRateServiceConfig::default())
            .with_cache(cache)
            .add_provider(rate_provider)
            .with_fee_service(fee_service);

    // Example 1: Get current exchange rate
    println!("Example 1: Get Current Exchange Rate");
    println!("-------------------------------------");
    let rate = exchange_service.get_rate("NGN", "cNGN").await?;
    println!("NGN -> cNGN rate: {}", rate);
    println!("✓ Rate fetched successfully\n");

    // Example 2: Calculate conversion with fees (Onramp)
    println!("Example 2: Calculate Onramp Conversion");
    println!("---------------------------------------");
    let onramp_request = ConversionRequest {
        from_currency: "NGN".to_string(),
        to_currency: "cNGN".to_string(),
        amount: BigDecimal::from(50000),
        direction: ConversionDirection::Buy,
    };

    let onramp_result = exchange_service
        .calculate_conversion(onramp_request)
        .await?;

    println!(
        "User pays: {} {}",
        onramp_result.from_amount, onramp_result.from_currency
    );
    println!("Base rate: {}", onramp_result.base_rate);
    println!(
        "Gross amount: {} {}",
        onramp_result.gross_amount, onramp_result.to_currency
    );
    println!(
        "Provider fee: {} {}",
        onramp_result.fees.provider_fee, onramp_result.to_currency
    );
    println!(
        "Platform fee: {} {}",
        onramp_result.fees.platform_fee, onramp_result.to_currency
    );
    println!(
        "Total fees: {} {}",
        onramp_result.fees.total_fees, onramp_result.to_currency
    );
    println!(
        "User receives: {} {}",
        onramp_result.net_amount, onramp_result.to_currency
    );
    println!("Quote expires at: {}", onramp_result.expires_at);
    println!("✓ Onramp conversion calculated\n");

    // Example 3: Calculate conversion with fees (Offramp)
    println!("Example 3: Calculate Offramp Conversion");
    println!("----------------------------------------");
    let offramp_request = ConversionRequest {
        from_currency: "cNGN".to_string(),
        to_currency: "NGN".to_string(),
        amount: BigDecimal::from(50000),
        direction: ConversionDirection::Sell,
    };

    let offramp_result = exchange_service
        .calculate_conversion(offramp_request)
        .await?;

    println!(
        "User sells: {} {}",
        offramp_result.from_amount, offramp_result.from_currency
    );
    println!("Base rate: {}", offramp_result.base_rate);
    println!(
        "Gross amount: {} {}",
        offramp_result.gross_amount, offramp_result.to_currency
    );
    println!(
        "Provider fee: {} {}",
        offramp_result.fees.provider_fee, offramp_result.to_currency
    );
    println!(
        "Platform fee: {} {}",
        offramp_result.fees.platform_fee, offramp_result.to_currency
    );
    println!(
        "Total fees: {} {}",
        offramp_result.fees.total_fees, offramp_result.to_currency
    );
    println!(
        "User receives: {} {}",
        offramp_result.net_amount, offramp_result.to_currency
    );
    println!("Quote expires at: {}", offramp_result.expires_at);
    println!("✓ Offramp conversion calculated\n");

    // Example 4: Update exchange rate
    println!("Example 4: Update Exchange Rate");
    println!("--------------------------------");
    let new_rate = BigDecimal::from(1);
    exchange_service
        .update_rate("NGN", "cNGN", new_rate.clone(), "manual_update")
        .await?;
    println!("Updated NGN -> cNGN rate to: {}", new_rate);
    println!("✓ Rate updated successfully\n");

    // Example 5: Get historical rate
    println!("Example 5: Get Historical Rate");
    println!("-------------------------------");
    let timestamp = chrono::Utc::now();
    let historical_rate = exchange_service
        .get_historical_rate("NGN", "cNGN", timestamp)
        .await?;
    println!("Historical rate at {}: {}", timestamp, historical_rate);
    println!("✓ Historical rate retrieved\n");

    // Example 6: Cache invalidation
    println!("Example 6: Cache Invalidation");
    println!("------------------------------");
    exchange_service.invalidate_cache("NGN", "cNGN").await?;
    println!("Cache invalidated for NGN -> cNGN");
    println!("✓ Cache cleared successfully\n");

    println!("=== All Examples Completed Successfully ===");

    Ok(())
}

#[cfg(not(feature = "database"))]
fn main() {
    println!("This example requires the 'database' and 'cache' features to be enabled.");
    println!(
        "Run with: cargo run --example exchange_rate_service_example --features database,cache"
    );
}
