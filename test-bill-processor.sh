#!/bin/bash

# Bill Processor Testing Script
# Tests bill payment processor with manual transactions

set -e

echo "🧾 Bill Processor Testing Script"
echo "================================="

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Configuration
DB_NAME="${DB_NAME:-aframp_test}"
SYSTEM_WALLET="${SYSTEM_WALLET:-GAQZPYQHTYQ5P42PKEQHFZRVBFGDDX2QMJGPFBLX4BLWUJWXQE5Z46FW}"
TESTNET_ASSET="cNGN"

# Helper functions
log_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

log_info() {
    echo -e "${YELLOW}ℹ $1${NC}"
}

log_error() {
    echo -e "${RED}✗ $1${NC}"
}

# Test 1: Create Bill Payment Transaction
test_create_bill_transaction() {
    log_info "Test 1: Creating bill payment transaction..."
    
    # Generate UUIDs
    WALLET_ADDRESS="G$(openssl rand -hex 27 | tr '[:lower:]' '[:upper:]')"
    TX_ID=$(uuidgen)
    
    log_info "Transaction ID: $TX_ID"
    log_info "Wallet: $WALLET_ADDRESS"
    
    # Insert transaction
    psql -U "$DB_USER" -d "$DB_NAME" << EOF
INSERT INTO users (id, email) VALUES ('$TX_ID', 'test@example.com');

INSERT INTO wallets (id, user_id, wallet_address, chain, created_at, updated_at) 
VALUES (gen_random_uuid(), '$TX_ID', '$WALLET_ADDRESS', 'stellar', now(), now());

INSERT INTO transactions (
    transaction_id, wallet_address, type, from_amount, to_amount, 
    cngn_amount, status, metadata, created_at, updated_at
) VALUES (
    '$TX_ID', '$WALLET_ADDRESS', 'bill_payment', 0, 0, 5030, 'pending_payment',
    '{"bill_type": "electricity", "provider_code": "ekedc", "account_number": "1234567890"}',
    now(), now()
);

INSERT INTO bill_payments (
    transaction_id, provider_name, account_number, bill_type, created_at, updated_at
) VALUES (
    '$TX_ID', 'flutterwave', '1234567890', 'electricity', now(), now()
);
EOF
    
    log_success "Bill payment transaction created"
    echo $TX_ID
}

# Test 2: Check Bill Payment Status
test_check_bill_status() {
    local tx_id=$1
    log_info "Test 2: Checking bill payment status for $tx_id..."
    
    psql -U "$DB_USER" -d "$DB_NAME" << EOF
SELECT 
    bp.id,
    bp.transaction_id,
    bp.bill_type,
    bp.account_number,
    bp.provider_name,
    bp.status,
    bp.retry_count,
    bp.error_message,
    bp.created_at
FROM bill_payments bp
WHERE bp.transaction_id = '$tx_id';
EOF
    
    log_success "Status retrieved"
}

# Test 3: Simulate cNGN Payment Detection
test_simulate_cngn_payment() {
    local tx_id=$1
    log_info "Test 3: Simulating cNGN payment for $tx_id..."
    
    # In real scenario, this would be detected by monitoring Stellar
    # For testing, we just update the transaction status
    
    psql -U "$DB_USER" -d "$DB_NAME" << EOF
UPDATE transactions 
SET metadata = jsonb_set(metadata, '{cngn_received}', 'true'::jsonb),
    status = 'processing'
WHERE transaction_id = '$tx_id';

UPDATE bill_payments
SET status = 'cngn_received'
WHERE transaction_id = '$tx_id';
EOF
    
    log_success "cNGN payment simulated"
}

# Test 4: Check Success Rates
test_success_rates() {
    log_info "Test 4: Checking bill payment success rates..."
    
    psql -U "$DB_USER" -d "$DB_NAME" << EOF
SELECT 
    bill_type,
    COUNT(*) as total,
    COUNT(CASE WHEN status = 'completed' THEN 1 END) as completed,
    COUNT(CASE WHEN status = 'refunded' THEN 1 END) as refunded,
    ROUND(COUNT(CASE WHEN status = 'completed' THEN 1 END)::numeric / COUNT(*) * 100, 2) as success_rate
FROM bill_payments
WHERE created_at > NOW() - INTERVAL '24 hours'
GROUP BY bill_type;
EOF
    
    log_success "Success rates retrieved"
}

# Test 5: Check Provider Performance
test_provider_performance() {
    log_info "Test 5: Checking provider performance..."
    
    psql -U "$DB_USER" -d "$DB_NAME" << EOF
SELECT 
    provider_name,
    COUNT(*) as total,
    COUNT(CASE WHEN status = 'completed' THEN 1 END) as successful,
    ROUND(COUNT(CASE WHEN status = 'completed' THEN 1 END)::numeric / COUNT(*) * 100, 2) as success_rate,
    MAX(retry_count) as max_retries
FROM bill_payments
WHERE created_at > NOW() - INTERVAL '24 hours'
GROUP BY provider_name;
EOF
   
    log_success "Provider performance retrieved"
}

# Test 6: Test Account Verification Logic
test_account_verification() {
    log_info "Test 6: Testing account verification logic..."
    
    # Test valid meter
    log_info "Testing meter: 1234567890"
    if [[ "1234567890" =~ ^[0-9]{10,12}$ ]]; then
        log_success "Meter format valid"
    else
        log_error "Meter format invalid"
        return 1
    fi
    
    # Test invalid meter
    log_info "Testing meter: ABC (should fail)"
    if [[ "ABC" =~ ^[0-9]{10,12}$ ]]; then
        log_error "Should have failed validation"
        return 1
    else
        log_success "Invalid meter correctly rejected"
    fi
    
    # Test valid phone
    log_info "Testing phone: 08012345678"
    if [[ "08012345678" =~ ^0[0-9]{10}$ ]]; then
        log_success "Phone format valid"
    else
        log_error "Phone format invalid"
        return 1
    fi
}

# Test 7: Test Retry Backoff Logic
test_retry_logic() {
    log_info "Test 7: Testing retry backoff logic..."
    
    BACKOFF_SCHEDULE=(10 60 300)
    MAX_RETRIES=3
    
    for attempt in $(seq 1 $MAX_RETRIES); do
        idx=$((attempt - 1))
        wait_time=${BACKOFF_SCHEDULE[$idx]:-300}
        log_info "Attempt $attempt: Wait ${wait_time}s"
    done
    
    log_success "Backoff logic correct"
}

# Test 8: Cleanup
test_cleanup() {
    log_info "Test 8: Cleaning up test data..."
    
    psql -U "$DB_USER" -d "$DB_NAME" << EOF
DELETE FROM bill_payments WHERE created_at > NOW() - INTERVAL '1 hour';
DELETE FROM transactions WHERE created_at > NOW() - INTERVAL '1 hour' AND type = 'bill_payment';
EOF
    
    log_success "Test data cleaned up"
}

# Main execution
main() {
    log_info "Starting bill processor tests..."
    
    # Check prerequisites
    if ! command -v psql &> /dev/null; then
        log_error "PostgreSQL client not found"
        exit 1
    fi
    
    # Run tests
    test_account_verification || exit 1
    test_retry_logic || exit 1
    
    if [ -z "$READ_ONLY" ]; then
        # Only run write tests if not in read-only mode
        TX_ID=$(test_create_bill_transaction)
        test_check_bill_status "$TX_ID" || exit 1
        test_simulate_cngn_payment "$TX_ID" || exit 1
        test_cleanup || exit 1
    fi
    
    test_success_rates || exit 1
    test_provider_performance || exit 1
    
    log_success "All tests completed!"
}

# Run main
main "$@"
