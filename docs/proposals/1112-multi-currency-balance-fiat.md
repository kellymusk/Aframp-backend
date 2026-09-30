# Enhancement: multi-currency balance display with NGN fiat equivalent

**Issue:** [#1112](https://github.com/kellymusk/Aframp-backend/issues/1112)  
**Component:** `GET /balance` — `src/api/balances.rs`, `src/services/balances.rs`  
**Priority:** Medium  
**Estimated effort:** ~3 hours

---

## Problem

`GET /balance` currently returns:

```json
[
  { "merchant_id": "...", "asset": "XLM", "available": 10000000, "pending": 0, "updated_at": "..." }
]
```

Merchants must mentally convert stroops to XLM (`÷ 10_000_000`) and then to
NGN using an exchange rate they have to look up themselves. This is friction
for everyday use — a merchant reconciling sales needs to see ₦12,450 NGN, not
`10000000`.

---

## Proposed API change

Add an optional query parameter:

```
GET /balance?include_fiat=true
```

When present, each balance entry includes `ngn_equivalent`:

```json
[
  {
    "merchant_id": "...",
    "asset": "XLM",
    "available": 10000000,
    "pending": 0,
    "updated_at": "...",
    "ngn_equivalent": {
      "available": 12450,
      "pending": 0,
      "rate_per_unit": 1245.0,
      "rate_source": "coingecko",
      "rate_cached_at": "2024-01-15T10:00:00Z"
    }
  }
]
```

When `include_fiat` is absent or `false`, the response is identical to today —
no breaking change.

---

## Architecture

### Exchange rate provider trait

Mirror the `PaymentProvider` pattern:

```rust
// src/services/exchange_rates.rs  (NEW)
use async_trait::async_trait;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ExchangeRate {
    pub asset: String,
    pub ngn_per_unit: f64,
    pub source: String,
    pub fetched_at: chrono::DateTime<chrono::Utc>,
}

#[async_trait]
pub trait ExchangeRateProvider: Send + Sync {
    /// Fetch current NGN rates for the given asset symbols.
    /// Returns a map from asset symbol → rate.
    async fn get_rates(
        &self,
        assets: &[String],
    ) -> Result<HashMap<String, ExchangeRate>, String>;
}
```

### CoinGecko implementation

```rust
// src/services/coingecko.rs  (NEW)
pub struct CoinGeckoProvider {
    client: reqwest::Client,
    cache: tokio::sync::Mutex<RateCache>,
}

struct RateCache {
    rates: HashMap<String, ExchangeRate>,
    fetched_at: std::time::Instant,
}

impl CoinGeckoProvider {
    const CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes
    const API_URL: &'static str =
        "https://api.coingecko.com/api/v3/simple/price?ids={ids}&vs_currencies=ngn";
}
```

CoinGecko's public API does not require an API key for the `/simple/price`
endpoint (rate-limited to ~30 req/min, sufficient for a 5-minute cache).

Asset ID mappings:
| Aframp asset | CoinGecko ID  |
|--------------|---------------|
| `XLM`        | `stellar`     |
| `cNGN`       | `cngn`        |

### Mock implementation (for tests)

```rust
// src/services/exchange_rates_mock.rs  (NEW)
pub struct MockExchangeRateProvider {
    pub rates: HashMap<String, f64>, // asset → NGN/unit
}

#[async_trait]
impl ExchangeRateProvider for MockExchangeRateProvider {
    async fn get_rates(&self, assets: &[String]) -> Result<HashMap<String, ExchangeRate>, String> {
        // return fixed rates from self.rates
    }
}
```

### AppState change

```rust
// src/lib.rs
pub struct AppState {
    // ... existing fields ...
    pub exchange_rate_provider: Option<Arc<dyn ExchangeRateProvider>>,
}
```

`None` = fiat display not available (default); `Some(...)` = enabled.

### Balance handler update

```rust
// src/api/balances.rs
pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<BalanceQuery>,
) -> ApiResult<Json<Vec<BalanceWithFiat>>> {
    // ...
    if params.include_fiat.unwrap_or(false) {
        // fetch rates, compute ngn_equivalent
    }
}
```

---

## Acceptance criteria

1. `GET /balance` without `?include_fiat` returns the existing shape (no regression)
2. `GET /balance?include_fiat=true` returns each balance with `ngn_equivalent`
3. Exchange rates are cached for 5 minutes (verified in tests via a mock provider)
4. If the rate provider fails, the endpoint returns balances without
   `ngn_equivalent` and logs a warning — it does not return a 500
5. Integration tests use a `MockExchangeRateProvider` with fixed rates

The specification tests for criteria 3–5 live in
`tests/balance_fiat_display.rs` (provided in this PR).

---

## Notes

- No database migration needed — this is a pure computation layer.
- CoinGecko public API: `https://api.coingecko.com/api/v3/simple/price`
- cNGN is pegged 1:1 to NGN, so its rate would always be ≈ 1.0 NGN/cNGN
  unless it drifts — still worth fetching rather than hardcoding.
- Binance public API (`https://api.binance.com/api/v3/ticker/price`) is an
  alternative if CoinGecko rate limits become an issue.
