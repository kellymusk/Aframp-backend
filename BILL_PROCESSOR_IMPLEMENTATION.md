# Bill Payment Service Integration - Implementation Guide

## Overview

The Bill Payment Service is a background worker that automates bill payments for Nigerian utilities. It:

1. Monitors for incoming cNGN payments with BILL-* memos
2. Verifies account validity before processing
3. Executes payments through provider APIs (Flutterwave, VTPass, Paystack)
4. Handles failures with automatic retry and exponential backoff
5. Processes refunds on permanent failure
6. Delivers electricity tokens and sends notifications

## Architecture

### Components

#### 1. Bill Processor Worker (`bill_processor.rs`)
- Background task running every 10 seconds
- Orchestrates the complete bill payment lifecycle
- Manages state transitions and retry logic

#### 2. Provider Adapters
- **FlutterwaveAdapter**: Electricity, airtime, data, cable TV
- **VTPassAdapter**: All utilities (comprehensive coverage)
- **PaystackAdapter**: Limited utility support

#### 3. Account Verification (`account_verification.rs`)
- Pre-payment validation
- Bill-type specific verification
- Phone number and meter validation

#### 4. Payment Executor (`payment_executor.rs`)
- Direct payment processing
- Retry logic with exponential backoff
- Status tracking

#### 5. Refund Handler (`refund_handler.rs`)
- Determines refund eligibility
- Processes cNGN refunds to user wallets
- Tracks refund completion

#### 6. Token Manager (`token_manager.rs`)
- Formats and validates tokens
- Stores tokens in database
- Generates notification messages

## Processing Pipeline

### State Diagram

```
pending_payment
    ↓
cngn_received
    ↓
verifying_account ──→ account_invalid
    ↓                      ↓
processing_bill       refund_initiated
    ↓                      ↓
provider_processing   refund_processing
    ↓                      ↓
completed or          refunded
retry_scheduled
```

### Stage 1: Receipt Verification

When cNGN is detected in system wallet with BILL-* memo:
1. Extract transaction from blockchain
2. Match to bill payment transaction in database
3. Verify amount matches exactly
4. If amount matches → transition to `verifying_account`
5. If amount mismatch → transition to `refund_initiated`

### Stage 2: Account Re-verification

For transactions in `verifying_account`:
1. Call provider verification API
2. Ensure account is active
3. For airtime/data: validate phone format
4. For electricity: validate meter exists
5. For cable TV: validate smart card
6. If valid → transition to `processing_bill`
7. If invalid → transition to `refund_initiated`

### Stage 3: Bill Payment Processing

For transactions in `processing_bill`:
1. Select primary provider based on bill type
2. Build payment request
3. Call provider payment API
4. Store provider reference and response
5. If success → transition to `provider_processing` or `completed`
6. If failure → transition to `retry_scheduled`

### Stage 4: Status Monitoring

For transactions in `provider_processing`:
1. Query provider status API using provider_reference
2. Check for token/confirmation
3. If completed → store token, transition to `completed`
4. If still pending → continue monitoring
5. If failed → transition to `retry_scheduled`

### Stage 5: Retry Processing

For transactions in `retry_scheduled`:
1. Check if time to retry based on backoff schedule
2. If ready → increment retry_count, transition to `processing_bill`
3. If max retries exceeded → transition to `refund_initiated`

Backoff schedule:
- Retry 1: After 10 seconds
- Retry 2: After 1 minute
- Retry 3: After 5 minutes
- Max retries: 3

### Stage 6: Refund Processing

For transactions in `refund_initiated`:
1. Build cNGN refund transaction on Stellar
2. Submit to blockchain
3. Transition to `refund_processing`
4. Wait for confirmation
5. Once confirmed → transition to `refunded`
6. Send refund notification

## Provider Selection

### Primary providers by bill type:
- **Electricity**: Flutterwave (best uptime)
- **Airtime**: VTPass (fastest processing)
- **Data**: VTPass (comprehensive plans)
- **Cable TV**: Flutterwave (good coverage)

### Failover chain:
1. Primary provider
2. Backup provider 1
3. Backup provider 2
4. If all fail → Refund

## Key Features

### 1. Account Verification

**Electricity Meters**
- Validate format: 10-12 digits
- Call provider API
- Check account is active
- Retrieve customer name and balance

**Airtime/Data**
- Validate phone: 080XXXXXXXX format
- Detect network from prefix
- Instant (no API call)

**Cable TV**
- Validate smart card: 9-12 digits
- Call provider API
- Check subscription active
- Get package info

### 2. Payment Execution

Each provider has specific request formats:

**Flutterwave**:
```json
{
  "customer": "1234567890",
  "amount": 5000,
  "recurrence": "ONCE",
  "type": "PREPAID",
  "reference": "tx_abc123"
}
```

**VTPass**:
```json
{
  "serviceID": "mtn",
  "phone": "08012345678",
  "amount": 500,
  "request_id": "tx_abc123"
}
```

### 3. Token Management

Electricity tokens are critical:
- Format: 1234-5678-9012-3456 or 12345678901234567890
- Store in database immediately
- Display in notifications
- User enters on meter to load electricity

Token retrieval if missing:
1. Wait 30 seconds
2. Query provider status endpoint
3. Extract and store token
4. Notify user
5. If no token after 5 minutes: Alert support

### 4. Retry Logic

- Exponential backoff (10s, 1m, 5m)
- Max 3 attempts
- Jitter to prevent thundering herd
- Network errors trigger immediate retry
- Provider errors trigger delayed retry

### 5. Refund System

Refunds triggered by:
- Amount mismatch
- Account verification failed
- Provider permanently unavailable
- Max retries exceeded
- User cancellation (future feature)

Refund process:
1. Mark as `refund_initiated`
2. Build Stellar refund transaction
3. Submit to Stellar network
4. Monitor confirmation
5. Update to `refunded` once confirmed
6. Send notification with refund details

## Configuration

### Environment Variables

```bash
# Worker settings
BILL_PROCESSOR_POLL_INTERVAL_SECONDS=10
BILL_PROCESSOR_BATCH_SIZE=50

# Flutterwave
FLUTTERWAVE_API_KEY=your_key
FLUTTERWAVE_SECRET_KEY=your_secret
FLUTTERWAVE_BASE_URL=https://api.flutterwave.com

# VTPass
VTPASS_API_KEY=your_key
VTPASS_SECRET_KEY=your_secret
VTPASS_BASE_URL=https://api.vtpass.com

# Paystack
PAYSTACK_API_KEY=your_key
PAYSTACK_BASE_URL=https://api.paystack.co
```

### Retry Configuration

Adjust in `BillProcessorConfig`:
```rust
pub retry_attempts: u32,           // Max attempts (default: 3)
pub retry_backoff_seconds: Vec<u64>, // [10, 60, 300]
pub token_retrieval_timeout: Duration,  // Default: 5 minutes
```

## Integration with Existing Systems

### 1. Transaction System
- Uses existing `transactions` table with type='bill_payment'
- Extends with `bill_payments` table for bill-specific data
- Metadata field stores bill payment state

### 2. Stellar Integration
- Uses existing `StellarClient` for refunds
- Builds transactions with memo "REFUND-{transaction_id}"
- Signs and submits to Stellar network

### 3. Notifications
- Integration point for email/SMS/push notifications
- Passes structured data with bill details and tokens
- Templates for electricity, airtime, cable TV, refunds

### 4. Database
- Stores transactions with metadata
- Tracks retry attempts and timing
- Stores provider responses and tokens
- Maintains audit trail

## Usage

### Starting the Worker

In `main.rs`:

```rust
// Initialize worker
let bill_processor_config = BillProcessorConfig::from_env();
let bill_processor = BillProcessorWorker::new(
    db_pool.clone(),
    stellar_client.clone(),
    bill_processor_config,
)?;

// Run in background
let worker_shutdown_tx_clone = worker_shutdown_tx.clone();
tokio::spawn(async move {
    bill_processor.run(worker_shutdown_rx_bill).await;
});
```

### Sending Notifications

After successful payment:
```rust
let notification = BillPaymentNotification {
    transaction_id: tx.transaction_id.to_string(),
    bill_type: "electricity".to_string(),
    amount: 5030,
    currency: "NGN".to_string(),
    account_number: "1234567890".to_string(),
    provider: "EKEDC".to_string(),
    token: Some("1234-5678-9012-3456".to_string()),
    status: "completed".to_string(),
    message: "Payment successful".to_string(),
    customer_email: Some("user@example.com".to_string()),
    customer_phone: Some("08012345678".to_string()),
};

// Send via notification service
notify_service.send(&notification).await?;
```

## Monitoring & Metrics

### Key Metrics

Track using metrics middleware:
- `bill_payments_total`: Total payments processed
- `bill_payments_success`: Successful payments
- `bill_payments_failed`: Failed payments  
- `bill_payments_refunded`: Refunds processed
- `bill_payment_duration_seconds`: Processing time
- `bill_tokens_delivered`: Electricity tokens delivered
- `bill_retry_count`: Retry attempts

### Alerts

Set up alerts for:
- Payment failure rate > 10%
- Token not delivered after 5 minutes
- Refund rate > 5%
- Provider API down
- Processing time > 10 minutes
- Retry failures for same transaction > 3 times

### Dashboards

Build dashboards showing:
- Real-time bill payment status
- Success rate by provider
- Success rate by category
- Popular bill types
- Token delivery rates
- Average processing time

## Testing

### Unit Tests

Test individual components:
```bash
cargo test bill_processor
```

### Integration Tests

Test end-to-end flow:
```bash
cargo test --test bill_payment_integration --features integration-tests
```

### Manual Testing

1. Create test bill transaction
2. Send cNGN to system wallet with BILL-* memo
3. Monitor worker logs
4. Verify provider API calls
5. Check token storage
6. Verify refund if using test account

## Error Handling

The system handles:
- Network timeouts
- Invalid provider responses
- Missing tokens
- Account verification failures
- Amount mismatches
- Provider rate limits
- Database errors
- Stellar network errors

All errors are logged with context and retried appropriately.

## Security Considerations

### API Keys
- Store in vault/secrets manager
- Rotate periodically
- Never log API keys
- Separate keys for test/production

### Data Protection
- Phone numbers: Encrypt for storage
- Meter numbers: Hash for logs
- Smart card numbers: Secure storage
- Tokens: Don't store longer than necessary

### Duplicate Prevention
- Use transaction ID as idempotency key
- Check payment not already processed
- Lock transaction during processing
- Prevent concurrent processing of same transaction

## Notes

- Electricity tokens are critical—preserve them!
- Airtime is instant, electricity takes 2-5 minutes
- Cable TV may take 10-15 minutes to activate
- Some providers have daily downtime windows
- Monitor provider status pages
- Rate limit: respect provider API limits
- Test with small amounts first on real accounts
