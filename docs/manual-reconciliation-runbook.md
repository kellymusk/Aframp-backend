# Runbook: payout succeeded but DB write failed

Triggered by the log line
`payout succeeded but recording it failed; withdrawal needs manual reconciliation`
from `create_withdrawal_idempotent()` in `src/services/withdrawals.rs`.

Paystack accepted the transfer, but the withdrawal row is still `pending` with
no `provider_reference`. **Do not refund the merchant** — the money has left.

## 1. Gather details from the log

Note `withdrawal_id`, `transfer_code` and `provider` from the log entry.

## 2. Confirm the transfer with Paystack

```bash
curl -s https://api.paystack.co/transfer/<transfer_code> \
  -H "Authorization: Bearer $PAYSTACK_SECRET_KEY"
```

If the code is lost, list recent transfers and match on amount/recipient/reference:

```bash
curl -s "https://api.paystack.co/transfer?perPage=100" \
  -H "Authorization: Bearer $PAYSTACK_SECRET_KEY"
```

Check `data.status` (`success`, `pending`, `failed`, `reversed`).

## 3. Update the withdrawal row

```sql
BEGIN;
SELECT id, status, provider_reference FROM withdrawals WHERE id = '<withdrawal_id>' FOR UPDATE;
UPDATE withdrawals
   SET provider_reference = '<transfer_code>'
 WHERE id = '<withdrawal_id>' AND status = 'pending' AND provider_reference IS NULL;
COMMIT;
```

With `provider_reference` set, the normal webhook/status flow settles the row.
If Paystack reports `failed`/`reversed`, leave the row for the standard failure
path (which refunds the balance) rather than refunding by hand.

## 4. Record the incident

Log the withdrawal id, transfer code and the action taken in the ops channel.
