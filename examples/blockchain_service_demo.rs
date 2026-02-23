use std::sync::Arc;
use Bitmesh_backend::chains::stellar::client::StellarClient;
use Bitmesh_backend::chains::stellar::config::{StellarConfig, StellarNetwork};
use Bitmesh_backend::chains::stellar::service::StellarBlockchainService;
use Bitmesh_backend::chains::traits::{BlockchainService, MultiChainBalanceAggregator};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("🚀 Blockchain Service Demo\n");

    // Create Stellar service
    let stellar_config = StellarConfig {
        network: StellarNetwork::Testnet,
        ..Default::default()
    };

    let stellar_client = StellarClient::new(stellar_config)?;
    let stellar_service = StellarBlockchainService::new(stellar_client);

    println!(
        "✅ Initialized {} blockchain service\n",
        stellar_service.chain_id()
    );

    // Test address validation
    let test_address = "GCJRI5CIWK5IU67Q6DGA7QW52JDKRO7JEAHQKFNDUJUPEZGURDBX3LDX";
    println!("🔍 Validating address: {}", test_address);

    match stellar_service.validate_address(test_address) {
        Ok(_) => println!("✅ Address is valid\n"),
        Err(e) => println!("❌ Address validation failed: {}\n", e),
    }

    // Check if account exists
    println!("🔍 Checking if account exists...");
    match stellar_service.account_exists(test_address).await {
        Ok(exists) => {
            if exists {
                println!("✅ Account exists\n");

                // Get account details
                println!("📊 Fetching account details...");
                match stellar_service.get_account(test_address).await {
                    Ok(account) => {
                        println!("✅ Account Details:");
                        println!("   Address: {}", account.address);
                        println!("   Sequence: {}", account.sequence);
                        println!("   Balances: {}", account.balances.len());

                        for balance in &account.balances {
                            println!(
                                "   - {} {}: {}",
                                balance.asset_code,
                                balance.issuer.as_deref().unwrap_or("(native)"),
                                balance.balance
                            );
                        }
                        println!();
                    }
                    Err(e) => println!("❌ Failed to fetch account: {}\n", e),
                }

                // Get specific asset balance
                println!("💰 Checking XLM balance...");
                match stellar_service
                    .get_asset_balance(test_address, "XLM", None)
                    .await
                {
                    Ok(Some(balance)) => println!("✅ XLM Balance: {}\n", balance),
                    Ok(None) => println!("ℹ️  No XLM balance found\n"),
                    Err(e) => println!("❌ Failed to get balance: {}\n", e),
                }
            } else {
                println!("ℹ️  Account does not exist\n");
            }
        }
        Err(e) => println!("❌ Error checking account: {}\n", e),
    }

    // Health check
    println!("🏥 Performing health check...");
    match stellar_service.health_check().await {
        Ok(health) => {
            println!("✅ Health Check Results:");
            println!("   Chain: {}", health.chain_id);
            println!("   Healthy: {}", health.is_healthy);
            println!("   Response Time: {}ms", health.response_time_ms);
            println!("   Last Check: {}", health.last_check);
            if let Some(error) = health.error_message {
                println!("   Error: {}", error);
            }
            println!();
        }
        Err(e) => println!("❌ Health check failed: {}\n", e),
    }

    // Multi-chain aggregator demo
    println!("🌐 Multi-Chain Balance Aggregator Demo");
    let chains: Vec<Arc<dyn BlockchainService>> = vec![Arc::new(stellar_service)];

    let aggregator = MultiChainBalanceAggregator::new(chains);

    println!("📊 Fetching balances across all chains...");
    let all_balances = aggregator.get_all_balances(test_address).await;

    for (chain_id, result) in all_balances {
        match result {
            Ok(balances) => {
                println!("✅ {} chain: {} balances", chain_id, balances.len());
                for balance in balances {
                    println!("   - {}: {}", balance.asset_code, balance.balance);
                }
            }
            Err(e) => println!("❌ {} chain error: {}", chain_id, e),
        }
    }
    println!();

    // Health check all chains
    println!("🏥 Checking health of all chains...");
    let health_results = aggregator.health_check_all().await;

    for (chain_id, health) in health_results {
        println!(
            "   {} - Healthy: {} ({}ms)",
            chain_id, health.is_healthy, health.response_time_ms
        );
    }

    println!("\n✅ Demo complete!");

    Ok(())
}
