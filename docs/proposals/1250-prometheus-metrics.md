# Enhancement: Prometheus metrics endpoint

**Issue:** [#1250](https://github.com/kellymusk/Aframp-backend/issues/1250)
**Component:** `src/main.rs`, `src/blockchain/worker.rs`, `src/api/auth.rs`, `Cargo.toml`

## Crates

`metrics = "0.24"` and `metrics-exporter-prometheus = "0.16"` (no default
features). The facade keeps call sites one-liners; the exporter renders text
format.

## Configuration

| Var | Default | Meaning |
|-----|---------|---------|
| `APP_METRICS_ADDR` | unset (disabled) | e.g. `127.0.0.1:9100`. When set, serve `GET /metrics` on this separate listener. |

The metrics listener is never merged into the public router, so `/metrics` is
reachable only by a scraper on the private network.

```rust
if let Some(addr) = config.metrics_addr {
    let handle = PrometheusBuilder::new().install_recorder()?;
    let app = Router::new().route("/metrics", get(move || async move { handle.render() }));
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await
    });
}
```

## Instruments

| Metric | Type | Where |
|--------|------|-------|
| `http_requests_total{method,route,status}` | counter | middleware layer (use `MatchedPath`, never the raw URI) |
| `http_request_duration_seconds{method,route}` | histogram | same layer |
| `deposit_poll_wallets` | gauge | worker loop, wallets polled this cycle |
| `deposit_poll_duration_seconds` | histogram | worker loop, cycle duration |
| `auth_requests_total{endpoint,outcome}` | counter | signup / login / verify-otp / refresh |
| `otp_challenges_total{event="created"\|"verified"}` | counter | OTP service |
| `withdrawals_total{outcome="success"\|"failed"}` | counter | withdrawals service |
| `balance_total_stroops{asset,kind="available"\|"pending"}` | gauge | refreshed each worker cycle |

Labels never include user, merchant or wallet ids (unbounded cardinality and PII).

## Tests

With `APP_METRICS_ADDR` unset, `GET /metrics` on the public router is `404`.
With a recorder installed, a login increments `auth_requests_total`.
