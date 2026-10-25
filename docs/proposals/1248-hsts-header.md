# Security: send `Strict-Transport-Security` from the app

**Issue:** [#1248](https://github.com/kellymusk/Aframp-backend/issues/1248)
**Component:** `src/main.rs`, `Cargo.toml`

## Problem

HSTS is only set by the sample Caddy config. Deployed without that proxy, the
app never tells browsers to stick to HTTPS.

## Proposed change

Enable the `set-header` feature of `tower-http` and add, in `main.rs`:

```rust
use tower_http::set_header::SetResponseHeaderLayer;

let hsts = config.cookie_secure.then(|| {
    SetResponseHeaderLayer::if_not_present(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    )
});

let app = router(state)
    .layer(cors)
    .layer(tower::util::option_layer(hsts))
    // ...
```

- `COOKIE_SECURE=true` (default, production) → header on every response.
- `COOKIE_SECURE=false` (plain-HTTP dev) → no header, so a dev browser is never
  pinned to HTTPS for the host.
- `if_not_present` lets a proxy that already sets HSTS keep its value.

## Tests

Assert the header is present when `cookie_secure` is true and absent when
false, by building the layered app as `tests/cors_preflight.rs` does for CORS.

Documented in `README.md` under "Deploying behind TLS".
