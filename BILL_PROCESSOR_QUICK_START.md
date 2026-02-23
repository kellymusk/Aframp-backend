# Bill Payment Processor - Quick Start Guide

## What It Does

The Bill Payment Processor automates bill payments for Nigerian utilities:

1. **Electricity** - Delivers tokens instantly to user meter
2. **Airtime** - Credits phone number immediately
3. **Data** - Activates data bundle on user's line
4. **Cable TV** - Renews subscription (DSTV, Startimes, etc.)

## Getting Started

### 1. Setup Environment Variables

```bash
# Flutterwave Configuration
export FLUTTERWAVE_API_KEY="your_flutterwave_secret_key"
export FLUTTERWAVE_SECRET_KEY="your_flutterwave_secret"
export FLUTTERWAVE_BASE_URL="https://api.flutterwave.com"

# VTPass Configuration  
export VTPASS_API_KEY="your_vtpass_api_key"
export VTPASS_SECRET_KEY="your_vtpass_secret_key"
export VTPASS_BASE_URL="https://api.vtpass.com"

# Paystack Configuration
export PAYSTACK_API_KEY="your_paystack_secret_key"
export PAYSTACK_BASE_URL="https://api.paystack.co"

# Bill Processor Settings
export BILL_PROCESSOR_POLL_INTERVAL_SECONDS="10"
export BILL_PROCESSOR_BATCH_SIZE="50"
```

### 2. Initialize the Worker in main.rs

```rust
use workers::bill_processor::{BillProcessorWorker, BillProcessorConfig};

// In your main function:
let bill_config = BillProcessorConfig::from_env();
let bill_processor = BillProcessorWorker::new(
    db_pool.clone(),
    stellar_client.clone(),
    bill_config,
)?;

// Create shutdown channel for the worker
let (bill_shutdown_tx, bill_shutdown_rx) = watch::channel(false);

// Spawn worker
tokio::spawn(async move {
    bill_processor.run(bill_shutdown_rx).await;
});

// Shutdown on app termination
// ... register bill_shutdown_tx in shutdown handler
```

### 3. Create Bill Payment Transaction

When a user initiates a bill payment, create a transaction:

```sql
-- Create transaction
INSERT INTO transactions (
    wallet_address, 
    type, 
    from_amount, 
    to_amount, 
    cngn_amount, 
    status, 
    metadata
)
VALUES (
    'G...',
    'bill_payment',
    0,
    0,
    5030,
    'pending_payment',
    jsonb_build_object(
        'bill_type', 'electricity',
        'provider_code', 'ekedc',
        'account_number', '1234567890',
        'account_type', 'PREPAID'
    )
)
RETURNING transaction_id;

-- Create bill payment details
INSERT INTO bill_payments (
    transaction_id,
    provider_name,
    account_number,
    bill_type
)
VALUES (transaction_id, 'flutterwave', '1234567890', 'electricity');
```

### 4. Send cNGN Payment

User sends CNGN to system_wallet with memo starting with "BILL-":

```
TO: system_wallet_address
AMOUNT: 5030 cNGN
ASSET: cNGN
MEMO: BILL-tx_uuid
```

### 5. Monitor Processing

The worker runs every 10 seconds and:

1. **Detects** incoming cNGN with BILL-* memo
2. **Verifies** meter/phone/card validity
3. **Processes** through provider API
4. **Stores** token in database
5. **Notifies** user with token/confirmation
6. **Refunds** on failure

## Example: Electricity Payment

```rust
// 1. User initiates electricity payment
let bill_type = "electricity";
let provider = "ekedc"; // EKEDC is primary provider
let meter = "1234567890";
let amount_ngn = 5030; // ₦5,030

// 2. Create transaction in database
// (Backend creates this when user clicks "Pay" in frontend)

// 3. Frontend instructs user to send cNGN
// User sends: 5030 cNGN to system_wallet with memo: BILL-{transaction_id}

// 4. Worker detects payment:
// - Matches to bill payment transaction
// - Verifies payment amount
// - Looks up meter with Flutterwave
// - Gets customer name and balance

// 5. Executes payment:
// - Call: POST /v3/bills/ekedc/payment
// - With: {"customer": "1234567890", "amount": 5000, ...}

// 6. Receives response:
// - Token: "1234-5678-9012-3456"
// - Reference: "FLW_REF_123"

// 7. Stores token and notifies user:
// Subject: Electricity Token - ₦5,000 EKEDC
// Your electricity payment was successful!
// 
// Meter: 1234567890
// Amount: ₦5,000
// Token: 1234-5678-9012-3456
// Enter this token on your meter to load electricity.

// 8. User enters token on meter
// ✓ Electricity loaded!
```

## Example: Airtime Payment

```rust
// 1. User buys ₦500 MTN airtime
let bill_type = "airtime";
let phone = "08012345678";
let amount_ngn = 500;

// 2. Transaction created in database

// 3. User sends: 500 cNGN to system_wallet with BILL-memo

// 4. Worker detects and processes:
// - Validates phone format
// - Detects network: MTN
// - Calls VTPass API

// 5. VTPass processes payment
// - Instant delivery
// - Returns: status = "delivered"

// 6. User receives notification:
// Subject: Airtime Delivered - ₦500 MTN
// Your airtime has been delivered!
// 
// Phone: 080****5678
// Amount: ₦500 MTN Airtime
// Status: Delivered

// 7. User's phone receives SMS with airtime credit
// ✓ Airtime balance updated!
```

## Example: Data Bundle

```rust
// 1. User buys 1GB GLO data
let bill_type = "data";
let phone = "07012345678"; // GLO number
let plan = "1GB"; // 1GB monthly plan
let amount_ngn = 300;

// 2-4. Transaction created and cNGN sent

// 5. Worker execution:
// - Validates phone
// - Detects network: Glo
// - Looks up data plans
// - Calls VTPass with variation_code

// 6. Data bundle activated
// - User's line receives notification
// - Data accessible immediately

// 7. Notification sent:
// Subject: Data Activated - 1GB GLO
// Your data bundle is now active!
// 
// Phone: 070****5678
// Plan: 1GB
// Valid for: 30 days
```

## Example: Cable TV Renewal

```rust
// 1. User renews DSTV subscription
let bill_type = "cable_tv";
let smart_card = "1234567890";
let package = "dstv-compact";
let amount_ngn = 7400;

// 2-4. Transaction created and cNGN sent

// 5. Worker execution:
// - Validates smart card
// - Looks up subscription
// - Calls Flutterwave with package
// - Poll for confirmation

// 6. After processing (10-15 minutes):
// - Subscription renewed
// - Decoder becomes active
// - Channels restored

// 7. User receives notification:
// Subject: DSTV Subscription Renewed
// Your DSTV subscription has been renewed!
// 
// Smart Card: 123****890
// Package: DSTV Compact
// Amount: ₦7,400
// Valid Until: March 18, 2026
// Your decoder should be active within 5 minutes.
```

## Error Scenarios

### Scenario 1: Invalid Meter Number

```
User pays: ₦5,000 for EKEDC
Meter: 9999999999 (invalid)

Process:
1. Worker verifies meter with EKEDC
2. Gets error: "Meter not found"
3. Transitions to: refund_initiated
4. Refunds ₦5,000 cNGN to user wallet

Notification:
Subject: Bill Payment Failed - Refund Processed
We couldn't complete your bill payment.

Provider: EKEDC
Account: 9999999999
Amount Refunded: 5,000 cNGN
Reason: Meter number not found

Your cNGN has been refunded to your wallet.
Please verify your meter number and try again.
```

### Scenario 2: Provider Timeout (Retry)

```
User pays: ₦500 MTN airtime
Provider: VTPass API times out

Process:
1. First attempt fails (timeout)
2. Wait 10 seconds
3. Retry 1: Still pending
4. Wait 60 seconds
5. Retry 2: Success!
6. Status: completed
7. Token: VT123456
8. Notification sent

Result: User gets airtime after 2 minutes
```

### Scenario 3: Max Retries (Refund)

```
User pays: ₦1,000 for cable TV
Provider: Service offline

Process:
1. Attempt 1: Provider error
2. Wait 10s → Retry
3. Attempt 2: Provider still offline
4. Wait 1m → Retry
5. Attempt 3: Provider still offline
6. Max retries (3) exceeded
7. Transition to: refund_initiated
8. Refund ₦1,000 to user wallet

Notification:
We couldn't complete your bill payment after multiple attempts.
Your ₦1,000 has been refunded.
Please try again later.
```

## Testing

### Manual Testing

1. **Setup test environment**:
   ```bash
   export SKIP_EXTERNALS=false
   export BILL_PROCESSOR_POLL_INTERVAL_SECONDS=5  # Faster polling
   cargo run
   ```

2. **Create test transaction**:
   ```sql
   INSERT INTO transactions (...) VALUES (...);
   INSERT INTO bill_payments (...) VALUES (...);
   ```

3. **Send test cNGN**:
   - Use Stellar CLI or SDK
   - Send to system wallet with BILL-memo
   - Monitor worker logs

4. **Verify results**:
   - Check transaction status
   - Verify token stored
   - Check notification sent

### Run Tests

```bash
# Unit tests
cargo test bill_processor::

# Integration tests
cargo test --test bill_processor_integration

# With logging
RUST_LOG=debug cargo test bill_processor::
```

## Monitoring

### Check Active Payments

```sql
SELECT status, COUNT(*) 
FROM bill_payments 
WHERE created_at > NOW() - INTERVAL '24 hours'
GROUP BY status;
```

### View Success Rates

```sql
SELECT 
    bill_type,
    COUNT(*) as total,
    COUNT(CASE WHEN status = 'completed' THEN 1 END) as completed,
    COUNT(CASE WHEN status = 'completed' THEN 1 END)::float / COUNT(*) as success_rate
FROM bill_payments
WHERE created_at > NOW() - INTERVAL '24 hours'
GROUP BY bill_type;
```

### Provider Performance

```sql
SELECT 
    provider_name,
    COUNT(CASE WHEN status = 'completed' THEN 1 END) as successful,
    COUNT(CASE WHEN status != 'completed' THEN 1 END) as failed,
    COUNT(CASE WHEN status = 'completed' THEN 1 END)::float / COUNT(*) as success_rate
FROM bill_payments
WHERE created_at > NOW() - INTERVAL '24 hours'
GROUP BY provider_name;
```

## Best Practices

1. **Always verify account before processing**
   - Meter must exist and be active
   - Phone must be valid format
   - Smart card must be current

2. **Handle tokens carefully**
   - Store immediately after receiving
   - Send in notification immediately
   - Never lose electricity tokens!
   - Keep backup of tokens

3. **Monitor provider status**
   - Check status pages regularly
   - Have fallback providers
   - Alert on high failure rates

4. **Test with small amounts first**
   - Start with ₦100 test payments
   - Verify full flow works
   - Then scale to production

5. **Keep audit trail**
   - Log all provider calls
   - Store full responses
   - Track retry attempts
   - Record refund reasons

6. **Secure API keys**
   - Use environment variables
   - Rotate periodically
   - Use different keys for test/prod
   - Never log API keys

## Troubleshooting

| Problem | Solution |
|---------|----------|
| Payments not processing | Check worker is running, verify logs |
| Tokens not received | Wait 5 min, check provider status |
| Amount mismatches | Verify cNGN amount matches transaction |
| Provider API errors | Check provider API keys and status |
| Refunds stuck | Check Stellar network connectivity |
| Phone validation fails | Verify 080XXXXXXXX format |
| Meter validation fails | Check meter number exists in DISCO system |

## Support

For issues or questions:
1. Check logs: `docker logs aframp-backend`
2. Review [BILL_PROCESSOR_IMPLEMENTATION.md](./BILL_PROCESSOR_IMPLEMENTATION.md)
3. Check provider API documentation
4. Contact provider support for account issues
