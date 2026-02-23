use sqlx::PgPool;
use std::str::FromStr;
use Bitmesh_backend::services::fee_calculation::FeeCalculationService;

type BigDecimal = sqlx::types::BigDecimal;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Connect to database
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost/aframp".to_string());

    let pool = PgPool::connect(&database_url).await?;

    // Create fee calculation service
    let service = FeeCalculationService::new(pool);

    println!("🧮 Aframp Fee Calculation Demo\n");
    println!("═══════════════════════════════════════════════════════════════\n");

    // Example 1: Small onramp transaction
    println!("📊 Example 1: Small Onramp Transaction");
    println!("─────────────────────────────────────");
    let amount1 = BigDecimal::from_str("10000").unwrap();
    let breakdown1 = service
        .calculate_fees("onramp", amount1.clone(), Some("flutterwave"), Some("card"))
        .await?;

    print_breakdown("Buy ₦10,000 worth of cNGN", &breakdown1);

    // Example 2: Medium onramp transaction
    println!("\n📊 Example 2: Medium Onramp Transaction");
    println!("─────────────────────────────────────");
    let amount2 = BigDecimal::from_str("100000").unwrap();
    let breakdown2 = service
        .calculate_fees("onramp", amount2.clone(), Some("flutterwave"), Some("card"))
        .await?;

    print_breakdown("Buy ₦100,000 worth of cNGN", &breakdown2);

    // Example 3: Large onramp transaction (fee cap applies)
    println!("\n📊 Example 3: Large Onramp Transaction (Fee Cap)");
    println!("─────────────────────────────────────");
    let amount3 = BigDecimal::from_str("1000000").unwrap();
    let breakdown3 = service
        .calculate_fees("onramp", amount3.clone(), Some("flutterwave"), Some("card"))
        .await?;

    print_breakdown("Buy ₦1,000,000 worth of cNGN", &breakdown3);

    // Example 4: Compare providers
    println!("\n📊 Example 4: Provider Comparison");
    println!("─────────────────────────────────────");
    let amount4 = BigDecimal::from_str("50000").unwrap();

    let flutterwave = service
        .calculate_fees("onramp", amount4.clone(), Some("flutterwave"), Some("card"))
        .await?;

    let paystack = service
        .calculate_fees("onramp", amount4.clone(), Some("paystack"), Some("card"))
        .await?;

    println!("Amount: ₦50,000\n");
    println!("Flutterwave:");
    println!("  Total fees: ₦{}", flutterwave.total);
    println!("  Effective rate: {}%", flutterwave.effective_rate);
    println!("\nPaystack:");
    println!("  Total fees: ₦{}", paystack.total);
    println!("  Effective rate: {}%", paystack.effective_rate);

    if flutterwave.total < paystack.total {
        let savings = &paystack.total - &flutterwave.total;
        println!("\n💡 Recommendation: Use Flutterwave (save ₦{})", savings);
    } else {
        let savings = &flutterwave.total - &paystack.total;
        println!("\n💡 Recommendation: Use Paystack (save ₦{})", savings);
    }

    // Example 5: Offramp transaction
    println!("\n\n📊 Example 5: Offramp Transaction");
    println!("─────────────────────────────────────");
    let amount5 = BigDecimal::from_str("100000").unwrap();
    let breakdown5 = service
        .calculate_fees(
            "offramp",
            amount5.clone(),
            Some("flutterwave"),
            Some("bank_transfer"),
        )
        .await?;

    print_breakdown("Sell 100,000 cNGN for NGN", &breakdown5);

    // Example 6: Fee estimation
    println!("\n\n📊 Example 6: Fee Estimation (No Provider Selected)");
    println!("─────────────────────────────────────");
    let amount6 = BigDecimal::from_str("75000").unwrap();
    let (min_fee, max_fee) = service.estimate_fees("onramp", amount6.clone()).await?;

    println!("Amount: ₦75,000");
    println!("Estimated fee range: ₦{} - ₦{}", min_fee, max_fee);
    println!("💡 Select a provider to see exact fees");

    println!("\n═══════════════════════════════════════════════════════════════");
    println!("✅ Demo completed successfully!");

    Ok(())
}

fn print_breakdown(
    title: &str,
    breakdown: &Bitmesh_backend::services::fee_calculation::FeeBreakdown,
) {
    println!("{}\n", title);
    println!("Amount: ₦{}", breakdown.amount);

    if let Some(provider) = &breakdown.provider {
        println!("\nProvider Fee ({} - {}):", provider.name, provider.method);
        println!("  Rate: {}%", provider.percent);
        if provider.flat > BigDecimal::from_str("0").unwrap() {
            println!("  Flat: ₦{}", provider.flat);
        }
        if let Some(cap) = &provider.cap {
            println!("  Cap: ₦{}", cap);
        }
        println!("  Calculated: ₦{}", provider.calculated);
    }

    println!("\nPlatform Fee:");
    println!("  Rate: {}%", breakdown.platform.percent);
    println!("  Calculated: ₦{}", breakdown.platform.calculated);

    println!("\nStellar Network Fee:");
    println!("  XLM: {}", breakdown.stellar.xlm);
    println!("  NGN: ₦{} (absorbed)", breakdown.stellar.ngn);

    println!("\n───────────────────────────────────");
    println!("Total Fees: ₦{}", breakdown.total);
    println!("Net Amount: ₦{}", breakdown.net_amount);
    println!("Effective Rate: {}%", breakdown.effective_rate);
}
