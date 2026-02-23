use sqlx::PgPool;
use std::str::FromStr;
use Bitmesh_backend::services::fee_calculation::{FeeBreakdown, FeeCalculationService};

type BigDecimal = sqlx::types::BigDecimal;

async fn setup_test_db() -> PgPool {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost/aframp_test".to_string());

    PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to test database")
}

async fn seed_fee_structures(pool: &PgPool) {
    // Clear existing test data
    sqlx::query("DELETE FROM fee_structures WHERE transaction_type LIKE 'test_%'")
        .execute(pool)
        .await
        .unwrap();

    // Tier 1: Small amounts (₦1,000 - ₦50,000)
    sqlx::query(
        r#"
        INSERT INTO fee_structures 
        (transaction_type, payment_provider, payment_method, min_amount, max_amount,
         provider_fee_percent, provider_fee_flat, provider_fee_cap, platform_fee_percent, is_active)
        VALUES ('onramp', 'flutterwave', 'card', 1000, 50000, 1.4, 100, 2000, 0.5, true)
        "#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Tier 2: Medium amounts (₦50,001 - ₦500,000)
    sqlx::query(
        r#"
        INSERT INTO fee_structures 
        (transaction_type, payment_provider, payment_method, min_amount, max_amount,
         provider_fee_percent, provider_fee_flat, provider_fee_cap, platform_fee_percent, is_active)
        VALUES ('onramp', 'flutterwave', 'card', 50001, 500000, 1.4, 0, 2000, 0.3, true)
        "#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Tier 3: Large amounts (₦500,001+)
    sqlx::query(
        r#"
        INSERT INTO fee_structures 
        (transaction_type, payment_provider, payment_method, min_amount, max_amount,
         provider_fee_percent, provider_fee_flat, provider_fee_cap, platform_fee_percent, is_active)
        VALUES ('onramp', 'flutterwave', 'card', 500001, NULL, 1.4, 0, 2000, 0.2, true)
        "#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Paystack fees
    sqlx::query(
        r#"
        INSERT INTO fee_structures 
        (transaction_type, payment_provider, payment_method, min_amount, max_amount,
         provider_fee_percent, provider_fee_flat, provider_fee_cap, platform_fee_percent, is_active)
        VALUES ('onramp', 'paystack', 'card', 1000, 50000, 1.5, 0, 2000, 0.5, true)
        "#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Offramp fees
    sqlx::query(
        r#"
        INSERT INTO fee_structures 
        (transaction_type, payment_provider, payment_method, min_amount, max_amount,
         provider_fee_percent, provider_fee_flat, provider_fee_cap, platform_fee_percent, is_active)
        VALUES ('offramp', 'flutterwave', 'bank_transfer', 1000, NULL, 0.8, 50, 5000, 0.5, true)
        "#,
    )
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn test_tier1_small_amount_fees() {
    let pool = setup_test_db().await;
    seed_fee_structures(&pool).await;

    let service = FeeCalculationService::new(pool);
    let amount = BigDecimal::from_str("10000").unwrap();

    let breakdown = service
        .calculate_fees("onramp", amount.clone(), Some("flutterwave"), Some("card"))
        .await
        .expect("Failed to calculate fees");

    // Provider fee: 10,000 × 1.4% + 100 = 140 + 100 = 240
    // Platform fee: 10,000 × 0.5% = 50
    // Total: 290
    assert_eq!(breakdown.amount, amount);
    assert!(breakdown.provider.is_some());

    let provider_fee = breakdown.provider.unwrap();
    assert_eq!(
        provider_fee.calculated,
        BigDecimal::from_str("240").unwrap()
    );
    assert_eq!(
        breakdown.platform.calculated,
        BigDecimal::from_str("50").unwrap()
    );
    assert_eq!(breakdown.total, BigDecimal::from_str("290").unwrap());
    assert_eq!(breakdown.net_amount, BigDecimal::from_str("9710").unwrap());
}

#[tokio::test]
async fn test_tier2_medium_amount_fees() {
    let pool = setup_test_db().await;
    seed_fee_structures(&pool).await;

    let service = FeeCalculationService::new(pool);
    let amount = BigDecimal::from_str("100000").unwrap();

    let breakdown = service
        .calculate_fees("onramp", amount.clone(), Some("flutterwave"), Some("card"))
        .await
        .expect("Failed to calculate fees");

    // Provider fee: 100,000 × 1.4% = 1,400 (no flat fee in tier 2)
    // Platform fee: 100,000 × 0.3% = 300
    // Total: 1,700
    let provider_fee = breakdown.provider.unwrap();
    assert_eq!(
        provider_fee.calculated,
        BigDecimal::from_str("1400").unwrap()
    );
    assert_eq!(
        breakdown.platform.calculated,
        BigDecimal::from_str("300").unwrap()
    );
    assert_eq!(breakdown.total, BigDecimal::from_str("1700").unwrap());
}

#[tokio::test]
async fn test_tier3_large_amount_with_cap() {
    let pool = setup_test_db().await;
    seed_fee_structures(&pool).await;

    let service = FeeCalculationService::new(pool);
    let amount = BigDecimal::from_str("1000000").unwrap();

    let breakdown = service
        .calculate_fees("onramp", amount.clone(), Some("flutterwave"), Some("card"))
        .await
        .expect("Failed to calculate fees");

    // Provider fee: 1,000,000 × 1.4% = 14,000 BUT capped at 2,000
    // Platform fee: 1,000,000 × 0.2% = 2,000
    // Total: 4,000
    let provider_fee = breakdown.provider.unwrap();
    assert_eq!(
        provider_fee.calculated,
        BigDecimal::from_str("2000").unwrap()
    );
    assert_eq!(
        breakdown.platform.calculated,
        BigDecimal::from_str("2000").unwrap()
    );
    assert_eq!(breakdown.total, BigDecimal::from_str("4000").unwrap());

    // Effective rate should be 0.4%
    let expected_rate = BigDecimal::from_str("0.4").unwrap();
    assert!(
        breakdown.effective_rate >= expected_rate
            && breakdown.effective_rate <= BigDecimal::from_str("0.41").unwrap()
    );
}

#[tokio::test]
async fn test_boundary_amount_tier_selection() {
    let pool = setup_test_db().await;
    seed_fee_structures(&pool).await;

    let service = FeeCalculationService::new(pool);

    // Test at tier boundary: 50,000 (should use tier 1)
    let breakdown1 = service
        .calculate_fees(
            "onramp",
            BigDecimal::from_str("50000").unwrap(),
            Some("flutterwave"),
            Some("card"),
        )
        .await
        .expect("Failed to calculate fees");

    // Should have flat fee (tier 1)
    let provider_fee1 = breakdown1.provider.unwrap();
    assert_eq!(provider_fee1.flat, BigDecimal::from_str("100").unwrap());

    // Test at tier boundary: 50,001 (should use tier 2)
    let breakdown2 = service
        .calculate_fees(
            "onramp",
            BigDecimal::from_str("50001").unwrap(),
            Some("flutterwave"),
            Some("card"),
        )
        .await
        .expect("Failed to calculate fees");

    // Should have no flat fee (tier 2)
    let provider_fee2 = breakdown2.provider.unwrap();
    assert_eq!(provider_fee2.flat, BigDecimal::from_str("0").unwrap());
}

#[tokio::test]
async fn test_paystack_vs_flutterwave_fees() {
    let pool = setup_test_db().await;
    seed_fee_structures(&pool).await;

    let service = FeeCalculationService::new(pool);
    let amount = BigDecimal::from_str("10000").unwrap();

    let flutterwave = service
        .calculate_fees("onramp", amount.clone(), Some("flutterwave"), Some("card"))
        .await
        .expect("Failed to calculate fees");

    let paystack = service
        .calculate_fees("onramp", amount.clone(), Some("paystack"), Some("card"))
        .await
        .expect("Failed to calculate fees");

    // Flutterwave: 1.4% + 100 = 240
    // Paystack: 1.5% = 150
    assert_eq!(
        flutterwave.provider.as_ref().unwrap().calculated,
        BigDecimal::from_str("240").unwrap()
    );
    assert_eq!(
        paystack.provider.as_ref().unwrap().calculated,
        BigDecimal::from_str("150").unwrap()
    );
}

#[tokio::test]
async fn test_offramp_fees() {
    let pool = setup_test_db().await;
    seed_fee_structures(&pool).await;

    let service = FeeCalculationService::new(pool);
    let amount = BigDecimal::from_str("100000").unwrap();

    let breakdown = service
        .calculate_fees(
            "offramp",
            amount.clone(),
            Some("flutterwave"),
            Some("bank_transfer"),
        )
        .await
        .expect("Failed to calculate fees");

    // Provider fee: 100,000 × 0.8% = 800 (min 50, max 5000)
    // Platform fee: 100,000 × 0.5% = 500
    // Total: 1,300
    let provider_fee = breakdown.provider.unwrap();
    assert_eq!(
        provider_fee.calculated,
        BigDecimal::from_str("800").unwrap()
    );
    assert_eq!(
        breakdown.platform.calculated,
        BigDecimal::from_str("500").unwrap()
    );
    assert_eq!(breakdown.total, BigDecimal::from_str("1300").unwrap());
}

#[tokio::test]
async fn test_fee_estimation() {
    let pool = setup_test_db().await;
    seed_fee_structures(&pool).await;

    let service = FeeCalculationService::new(pool);
    let amount = BigDecimal::from_str("10000").unwrap();

    let (min_fee, max_fee) = service
        .estimate_fees("onramp", amount)
        .await
        .expect("Failed to calculate fees");

    // Should return range based on different providers
    assert!(min_fee > BigDecimal::from_str("0").unwrap());
    assert!(max_fee >= min_fee);
}

#[tokio::test]
async fn test_stellar_fee_absorbed() {
    let pool = setup_test_db().await;
    seed_fee_structures(&pool).await;

    let service = FeeCalculationService::new(pool);
    let amount = BigDecimal::from_str("10000").unwrap();

    let breakdown = service
        .calculate_fees("onramp", amount, Some("flutterwave"), Some("card"))
        .await
        .expect("Failed to calculate fees");

    // Stellar fee should be absorbed (NGN = 0)
    assert_eq!(breakdown.stellar.ngn, BigDecimal::from_str("0").unwrap());
    assert!(breakdown.stellar.absorbed);
    assert_eq!(
        breakdown.stellar.xlm,
        BigDecimal::from_str("0.00001").unwrap()
    );
}

#[tokio::test]
async fn test_cache_invalidation() {
    let pool = setup_test_db().await;
    seed_fee_structures(&pool).await;

    let service = FeeCalculationService::new(pool);
    let amount = BigDecimal::from_str("10000").unwrap();

    // First call - loads from DB
    let _ = service
        .calculate_fees("onramp", amount.clone(), Some("flutterwave"), Some("card"))
        .await
        .expect("Failed to calculate fees");

    // Invalidate cache
    service.invalidate_cache().await;

    // Second call - should reload from DB
    let breakdown = service
        .calculate_fees("onramp", amount, Some("flutterwave"), Some("card"))
        .await
        .expect("Failed to calculate fees");

    assert!(breakdown.total > BigDecimal::from_str("0").unwrap());
}

#[tokio::test]
async fn test_effective_rate_calculation() {
    let pool = setup_test_db().await;
    seed_fee_structures(&pool).await;

    let service = FeeCalculationService::new(pool);

    // Test tier 1: ~2.9% effective rate
    let breakdown1 = service
        .calculate_fees(
            "onramp",
            BigDecimal::from_str("10000").unwrap(),
            Some("flutterwave"),
            Some("card"),
        )
        .await
        .expect("Failed to calculate fees");

    assert!(breakdown1.effective_rate >= BigDecimal::from_str("2.8").unwrap());
    assert!(breakdown1.effective_rate <= BigDecimal::from_str("3.0").unwrap());

    // Test tier 3: ~0.4% effective rate
    let breakdown3 = service
        .calculate_fees(
            "onramp",
            BigDecimal::from_str("1000000").unwrap(),
            Some("flutterwave"),
            Some("card"),
        )
        .await
        .expect("Failed to calculate fees");

    assert!(breakdown3.effective_rate >= BigDecimal::from_str("0.3").unwrap());
    assert!(breakdown3.effective_rate <= BigDecimal::from_str("0.5").unwrap());
}
