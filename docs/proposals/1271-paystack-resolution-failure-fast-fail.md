# Proposal: Paystack Account Resolution — Fast-Fail on Permanent Errors (#1271)

## Problem

In `src/payments/paystack.rs`, the account resolution step is currently
best-effort: any failure (network timeout, 404 account-not-found, etc.)
is swallowed and a placeholder name `"Aframp Merchant"` is used instead.

```rust
let resolved: Option<ResolvedAccount> = self
    .get("/bank/resolve", &[...])
    .await
    .map_err(|err| {
        tracing::warn!(error = %err, "account resolution failed, proceeding with a placeholder name");
        err
    })
    .ok();
let recipient_name = resolved
    .map(|r| r.account_name)
    .unwrap_or_else(|| "Aframp Merchant".to_string());
```

When the failure is **permanent** (e.g. the bank code / account number combination
does not exist — Paystack returns HTTP 422 or a structured error like
`"Could not resolve account name"`), the code creates a recipient against an
invalid account. Paystack may accept the recipient creation but then fail
the transfer, producing a confusing `502 PAYOUT_FAILED` that gives no signal
to the merchant that they need to correct their bank details.

## Proposed Fix

Distinguish transient errors (network timeout, 5xx from Paystack) from
permanent ones (4xx indicating the account doesn't exist) and return an
error immediately for permanent failures.

The key change is in `unwrap_response` and the `create_payout` logic:

```rust
/// Errors returned by Paystack, split by recoverability.
#[derive(Debug)]
enum PaystackError {
    /// Transient — worth retrying (network, 5xx).
    Transient(String),
    /// Permanent — the request is wrong and retrying will not help (4xx).
    Permanent(String),
}

impl PaystackError {
    fn is_permanent(&self) -> bool {
        matches!(self, PaystackError::Permanent(_))
    }
}

async fn unwrap_response<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, PaystackError> {
    let status = response.status();
    let raw = response.text().await.map_err(|e| PaystackError::Transient(e.to_string()))?;
    let body: PaystackResponse<T> = serde_json::from_str(&raw).map_err(|e| {
        PaystackError::Transient(format!(
            "Paystack returned an unexpected response (HTTP {status}): {e} — body: {raw}"
        ))
    })?;

    if !status.is_success() || !body.status {
        let msg = format!("Paystack error (HTTP {status}): {}", body.message);
        // 4xx from Paystack's resolve endpoint means the account doesn't exist —
        // there is nothing the platform can do; the merchant must provide
        // correct bank details.
        if status.is_client_error() {
            return Err(PaystackError::Permanent(msg));
        }
        return Err(PaystackError::Transient(msg));
    }

    body.data.ok_or_else(|| PaystackError::Transient("Paystack response missing data".to_string()))
}
```

And in `create_payout`:

```rust
// Resolve account name. Transient failures (network, 5xx) are tolerated
// and fall back to the placeholder. Permanent failures (4xx — the account
// or bank code doesn't exist) fail fast so the merchant sees a clear error
// rather than a confusing payout failure later.
let resolved: Option<ResolvedAccount> = match self
    .get("/bank/resolve", &[("account_number", ...), ("bank_code", ...)])
    .await
{
    Ok(r) => Some(r),
    Err(PaystackError::Permanent(msg)) => {
        // Return immediately — creating a recipient against this account
        // will either fail or produce a ghost recipient. The merchant
        // needs to correct their bank details.
        return Err(format!("account not found: {msg}"));
    }
    Err(PaystackError::Transient(msg)) => {
        tracing::warn!(
            error = %msg,
            "account resolution failed with a transient error, proceeding with placeholder name"
        );
        None
    }
};
let recipient_name = resolved
    .map(|r| r.account_name)
    .unwrap_or_else(|| "Aframp Merchant".to_string());
```

## Behaviour Changes

| Scenario | Before | After |
|---|---|---|
| Correct account number + bank code | Name resolved, payout proceeds | No change |
| Account doesn't exist (Paystack 422/404) | Placeholder used → payout likely fails with confusing error | `PAYOUT_FAILED` immediately with clear "account not found" message |
| Network timeout on resolve | Placeholder used → payout proceeds | No change (still tolerates transient errors) |
| Paystack 5xx on resolve | Placeholder used → payout proceeds | No change (still tolerates transient errors) |

## Test to Add

A test in `tests/withdrawal_flow.rs` (or a dedicated mock test) should verify that a
withdrawal attempt where the mock payment provider returns a 404-equivalent on account
resolution produces a `502 PAYOUT_FAILED` with a message that includes "account not found"
(or similar) rather than silently proceeding.

```rust
// Pseudo-code for the mock:
// MockProvider::create_payout: if account_number == "00000000000" => Err("account not found: ...")
// Then assert the withdrawal response is 502 with that message.
```

## References

- Issue: #1271
- `src/payments/paystack.rs` — the file to modify (protected; requires maintainer review)
- `src/payments/mock.rs` — mock provider used in tests
