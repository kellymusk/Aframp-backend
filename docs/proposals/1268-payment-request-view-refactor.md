# Proposal: Extract Payment Request View Transformation (#1268)

## Problem

In `src/api/payment_requests.rs`, view transformation is split across two
functions, `to_view()` and `row_to_view()`, both of which contain business
logic:

- `effective_status()` — derives the display status from a DB value and an
  expiry timestamp. This is a business rule (pending + expired → "expired"),
  not a formatting concern.
- `build_sep7_uri()` — constructs the SEP-0007 payment URI. This is also a
  business rule: what constitutes a valid URI depends on the asset type,
  amount encoding, and percent-encoding rules.

The list handler then performs a two-step transform:

```rust
rows.iter().map(row_to_view).collect()
```

which could be absorbed into the service layer's cursor query, eliminating
the need for a separate `row_to_view` pass in the handler.

## Proposed refactor

### 1. Move `build_sep7_uri` to `src/services/payment_requests.rs`

The SEP-7 URI is a function of the payment request's asset, amount, and memo.
The service layer already owns the `PaymentRequest` and `PaymentRequestWithWallet`
types; placing the URI builder there keeps business logic in one place.

```rust
// In src/services/payment_requests.rs
pub fn build_sep7_uri(address: &str, amount_stroops: i64, asset: &str, memo: &str) -> Option<String> {
    if asset != "XLM" && asset != "native" {
        return None;
    }
    let amount = format!("{}.{:07}", amount_stroops / 10_000_000, amount_stroops % 10_000_000);
    Some(format!(
        "web+stellar:pay?destination={}&amount={}&memo={}&memo_type=MEMO_TEXT",
        percent_encode(address),
        percent_encode(&amount),
        percent_encode(memo),
    ))
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
```

### 2. Move `effective_status` to `src/services/payment_requests.rs`

`effective_status` is already defined in the service module's context (it
operates on `PaymentRequestStatus` and `expires_at`). The API layer currently
re-defines it locally. Exposing it from the service module and deleting the
duplicate from `src/api/payment_requests.rs` removes the duplication.

```rust
// In src/services/payment_requests.rs
pub fn effective_status(status: PaymentRequestStatus, expires_at: chrono::DateTime<chrono::Utc>) -> String {
    if status == PaymentRequestStatus::Pending && expires_at < chrono::Utc::now() {
        "expired".to_string()
    } else {
        status.as_str().to_string()
    }
}
```

### 3. Reduce `to_view` and `row_to_view` to pure data mapping

Once `effective_status` and `build_sep7_uri` live in the service layer, the
view functions become trivial field mappings:

```rust
// In src/api/payment_requests.rs
fn to_view(pr: &PaymentRequest, address: &str, network: &str) -> PaymentRequestView {
    PaymentRequestView {
        id: pr.id,
        merchant_id: pr.merchant_id,
        address: address.to_string(),
        network: network.to_string(),
        amount_stroops: pr.amount_stroops,
        asset: pr.asset.clone(),
        memo: pr.memo.clone(),
        status: payment_requests::effective_status(pr.status, pr.expires_at),
        expires_at: pr.expires_at,
        cancelled_at: pr.cancelled_at,
        created_at: pr.created_at,
        sep7_uri: payment_requests::build_sep7_uri(address, pr.amount_stroops, &pr.asset, &pr.memo),
    }
}
```

### 4. (Optional) Inline the two-step `row_to_view` map

The list handler does:

```rust
rows.iter().map(row_to_view).collect()
```

This is already fine, but once `row_to_view` is a pure field mapping, its
body is short enough to inline at the call site or to express as part of the
service layer's cursor query return type, removing the function entirely.

## Migration path

1. Add `build_sep7_uri`, `percent_encode`, and `effective_status` to
   `src/services/payment_requests.rs` with `pub` visibility.
2. Update `src/api/payment_requests.rs` to call the service functions instead
   of the local ones (`payment_requests::build_sep7_uri(...)`,
   `payment_requests::effective_status(...)`).
3. Delete the local copies of `build_sep7_uri`, `percent_encode`, and
   `effective_status` from `src/api/payment_requests.rs`.
4. Verify `cargo build` and `cargo test` both pass.

The unit tests already in `src/api/payment_requests.rs` (`#[cfg(test)]` block)
test the encoding logic. After the move, those tests should live in
`src/services/payment_requests.rs` alongside the functions they cover.

## Test coverage

`tests/payment_request_view_transform_test.rs` documents the observable
behaviour of the transform via HTTP — that the JSON responses from
`GET /payment-requests`, `GET /payment-requests/{id}`, and
`GET /payment-requests/{id}/status` are correct after the refactor.

## Files changed (implementation)

| File | Change |
|------|--------|
| `src/services/payment_requests.rs` | Add `pub fn effective_status(...)`, `pub fn build_sep7_uri(...)`, `fn percent_encode(...)` |
| `src/api/payment_requests.rs` | Replace local calls with `payment_requests::*`; delete local copies |

No API surface, migration, or database schema changes.
