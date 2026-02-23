# Bill Payment API Endpoints

## Overview

Bill Payment endpoints allow users to pay bills using cNGN. The backend service (bill processor worker) handles all processing asynchronously.

## Endpoints

### 1. Get Supported Bill Types

**Endpoint**: `GET /api/bills/types`

**Description**: Returns list of supported bill types and providers

**Response**: 
```json
{
  "bill_types": [
    {
      "type": "electricity",
      "display_name": "Electricity",
      "providers": ["EKEDC", "IKEDC", "KEDCO", "BEDC"],
      "icon": "⚡",
      "description": "Pay for electricity tokens"
    },
    {
      "type": "airtime",
      "display_name": "Airtime",
      "providers": ["MTN", "Airtel", "Glo", "9Mobile"],
      "icon": "📱",
      "description": "Buy airtime credits"
    },
    {
      "type": "data",
      "display_name": "Data Bundle",
      "providers": ["MTN", "Airtel", "Glo", "9Mobile"],
      "icon": "📡",
      "description": "Buy data bundles"
    },
    {
      "type": "cable_tv",
      "display_name": "Cable TV",
      "providers": ["DSTV", "Startimes", "Mytv"],
      "icon": "📺",
      "description": "Renew cable TV subscription"
    }
  ]
}
```

### 2. Get Available Providers

**Endpoint**: `GET /api/bills/providers/:type`

**Parameters**: 
- `type` - Bill type (electricity, airtime, data, cable_tv)

**Response**:
```json
{
  "bill_type": "electricity",
  "providers": [
    {
      "code": "ekedc",
      "name": "EKEDC (Ikeja)",
      "region": "Lagos",
      "active": true
    },
    {
      "code": "ikedc",
      "name": "IKEDC (Ibadan)",
      "region": "Oyo",
      "active": true
    }
  ]
}
```

### 3. Validate Account (Pre-payment Verification)

**Endpoint**: `POST /api/bills/validate`

**Description**: Verify account before payment

**Request**:
```json
{
  "bill_type": "electricity",
  "provider_code": "ekedc",
  "account_number": "1234567890",
  "account_type": "PREPAID"
}
```

**Response**:
```json
{
  "valid": true,
  "account_info": {
    "account_number": "1234567890",
    "customer_name": "JOHN DOE",
    "status": "active",
    "outstanding_balance": 5230.50
  }
}
```

**Error Response** (invalid account):
```json
{
  "valid": false,
  "error": "Meter not found",
  "code": "ACCOUNT_NOT_FOUND"
}
```

### 4. Initiate Bill Payment

**Endpoint**: `POST /api/bills/pay`

**Description**: Create and initiate a bill payment transaction

**Request**:
```json
{
  "bill_type": "electricity",
  "provider_code": "ekedc",
  "account_number": "1234567890",
  "account_type": "PREPAID",
  "amount": 5030,
  "currency": "NGN"
}
```

**Response**:
```json
{
  "transaction_id": "550e8400-e29b-41d4-a716-446655440000",
  "status": "pending_payment",
  "amount": 5030,
  "currency": "NGN",
  "instructions": {
    "send_to": "GAQZPYQHTYQ5P42PKEQHFZRVBFGDDX2QMJGPFBLX4BLWUJWXQE5Z46FW",
    "amount_cngn": 5030,
    "memo": "BILL-550e8400-e29b-41d4-a716-446655440000",
    "asset": "cNGN",
    "timeout_seconds": 600
  },
  "expires_at": "2026-02-21T16:30:00Z"
}
```

### 5. Get Payment Status

**Endpoint**: `GET /api/bills/:transaction_id/status`

**Response** (pending):
```json
{
  "transaction_id": "550e8400-e29b-41d4-a716-446655440000",
  "status": "processing_bill",
  "stage": "Calling provider API (try 1/3)",
  "progress": 50,
  "updated_at": "2026-02-21T16:20:15Z"
}
```

**Response** (completed with token):
```json
{
  "transaction_id": "550e8400-e29b-41d4-a716-446655440000",
  "status": "completed",
  "completed_at": "2026-02-21T16:22:30Z",
  "bill_type": "electricity",
  "provider": "EKEDC",
  "token": "1234-5678-9012-3456",
  "instructions": "Enter this token on your meter to load electricity",
  "payment_details": {
    "amount": 5030,
    "provider_reference": "FLW_REF_123",
    "receipt_number": "RCP_123456"
  }
}
```

**Response** (failed with refund):
```json
{
  "transaction_id": "550e8400-e29b-41d4-a716-446655440000",
  "status": "refunded",
  "failed_at": "2026-02-21T16:25:00Z",
  "failure_reason": "Meter number not found",
  "refund": {
    "amount": 5030,
    "status": "completed",
    "transaction_hash": "hash_of_refund_tx",
    "timestamp": "2026-02-21T16:25:30Z"
  }
}
```

### 6. Get Payment History

**Endpoint**: `GET /api/bills/history`

**Query Parameters**:
- `limit` (default: 20)
- `offset` (default: 0)
- `status` (optional: completed, pending, failed, refunded)
- `bill_type` (optional: electricity, airtime, data, cable_tv)

**Response**:
```json
{
  "total": 45,
  "payments": [
    {
      "transaction_id": "550e8400-e29b-41d4-a716-446655440000",
      "bill_type": "electricity",
      "provider": "EKEDC",
      "amount": 5030,
      "status": "completed",
      "token": "1234-5678-9012-3456",
      "created_at": "2026-02-21T16:20:00Z",
      "completed_at": "2026-02-21T16:22:30Z"
    },
    {
      "transaction_id": "660e8400-e29b-41d4-a716-446655440001",
      "bill_type": "airtime",
      "provider": "MTN",
      "amount": 500,
      "status": "completed",
      "created_at": "2026-02-21T16:10:00Z",
      "completed_at": "2026-02-21T16:10:15Z"
    }
  ]
}
```

### 7. Get Bill Payment Savings/Stats

**Endpoint**: `GET /api/bills/stats`

**Response**:
```json
{
  "total_spent": 15000,
  "total_transactions": 12,
  "average_transaction": 1250,
  "categories": {
    "electricity": {
      "count": 6,
      "total": 7500,
      "average": 1250
    },
    "airtime": {
      "count": 4,
      "total": 4000,
      "average": 1000
    },
    "data": {
      "count": 2,
      "total": 3500,
      "average": 1750
    }
  },
  "recent_payments": [
    {
      "type": "electricity",
      "provider": "EKEDC",
      "amount": 5000,
      "date": "2026-02-21"
    },
    {
      "type": "airtime",
      "provider": "MTN",
      "amount": 1000,
      "date": "2026-02-20"
    }
  ]
}
```

## Error Codes

| Code | Status | Meaning |
|------|--------|---------|
| `INVALID_ACCOUNT` | 400 | Account number format invalid |
| `ACCOUNT_NOT_FOUND` | 404 | Account doesn't exist with provider |
| `ACCOUNT_INACTIVE` | 400 | Account is closed/inactive |
| `AMOUNT_INVALID` | 400 | Amount is invalid or below minimum |
| `AMOUNT_MISMATCH` | 400 | Amount doesn't match expected |
| `PROVIDER_ERROR` | 502 | Provider API error |
| `PROVIDER_TIMEOUT` | 504 | Provider API timeout |
| `INSUFFICIENT_BALANCE` | 400 | Provider wallet insufficient |
| `RATE_LIMIT` | 429 | Too many requests |
| `UNAUTHORIZED` | 401 | User not authenticated |
| `PAYMENT_EXPIRED` | 400 | Payment window expired |
| `TRANSACTION_NOT_FOUND` | 404 | Transaction doesn't exist |
| `INTERNAL_ERROR` | 500 | Internal server error |

## Status Values

- `pending_payment` - Waiting for cNGN payment
- `cngn_received` - cNGN received, verifying
- `verifying_account` - Validating account with provider
- `account_invalid` - Account failed verification
- `processing_bill` - Calling provider API
- `provider_processing` - Provider processing the payment
- `completed` - Payment successful, token issued
- `retry_scheduled` - Scheduled for retry
- `provider_failed` - Provider returned error
- `refund_initiated` - Refund being processed
- `refund_processing` - Refund in progress
- `refunded` - Refund completed to user

## Webhooks (Optional)

If webhook notifications are implemented:

**Bill Payment Completed**:
```json
{
  "event": "bill.payment.completed",
  "timestamp": "2026-02-21T16:22:30Z",
  "data": {
    "transaction_id": "550e8400-e29b-41d4-a716-446655440000",
    "bill_type": "electricity",
    "amount": 5030,
    "token": "1234-5678-9012-3456",
    "provider": "EKEDC"
  }
}
```

**Bill Payment Failed**:
```json
{
  "event": "bill.payment.failed",
  "timestamp": "2026-02-21T16:25:00Z",
  "data": {
    "transaction_id": "550e8400-e29b-41d4-a716-446655440000",
    "bill_type": "electricity",
    "amount": 5030,
    "reason": "Meter not found",
    "refund_amount": 5030,
    "refund_status": "completed"
  }
}
```

## Implementation Notes

1. **Async Processing**: All payments are processed asynchronously by the worker
2. **Status Polling**: Frontend should poll `/api/bills/:transaction_id/status` every 5-10 seconds
3. **Payment Expiration**: User has 10 minutes to send cNGN after initiating payment
4. **Validation**: Always validate account before payment
5. **Error Handling**: Always handle network errors gracefully
6. **Notifications**: Use email/SMS/push for payment confirmations and tokens
7. **Audit Trail**: All payments are logged for compliance

## Rate Limiting

- Per user: 100 payments per hour
- Per provider: 1000 payments per hour
- Per transaction: 3 retry attempts

## Security

- All requests require authentication (Bearer token)
- Phone numbers and meter numbers are encrypted
- Tokens should not be logged
- API keys must be rotated regularly
- Use HTTPS only
