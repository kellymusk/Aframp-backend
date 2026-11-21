# Proposal: `AuthMerchant` extractor (#1440)

## Problem
~15 handlers in withdrawals, payment requests, wallets and transactions repeat:

```rust
let merchant_id = auth.merchant_id.ok_or_else(|| {
    bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account")
})?;
```

## Proposal (maintainer change in `src/auth/extractor.rs`)

```rust
pub struct AuthMerchant {
    pub user: AuthUser,
    pub merchant_id: Uuid,
}

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for AuthMerchant {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        let merchant_id = user.merchant_id.ok_or_else(|| {
            bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account")
        })?;
        Ok(Self { user, merchant_id })
    }
}
```

Merchant-scoped handlers take `AuthMerchant` instead of `AuthUser`; the error
code and message live in one place. Response shape is unchanged.

## Tests
`tests/` should verify a session for a user with no merchant gets `400
MERCHANT_NOT_FOUND` from a merchant-scoped route, and a merchant user succeeds.
