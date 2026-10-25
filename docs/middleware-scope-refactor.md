# Middleware Scope Refactor — Design Note

**Issue:** [#1275](https://github.com/kellymusk/Aframp-backend/issues/1275)  
**Related source:** `src/lib.rs`, `src/middleware.rs`

---

## Problem

In `src/lib.rs`, `require_json_content_type` is applied as a blanket
router-level layer:

```rust
pub fn router(state: AppState) -> axum::Router {
    axum::Router::new()
        // … all routes …
        .with_state(state)
        .layer(axum::middleware::from_fn(middleware::require_json_content_type))
}
```

This layer runs on *every* request, including `GET /health` (a liveness probe)
and `GET /admin` (serves an HTML page, not JSON).

### Is it currently broken?

**No.** The middleware already short-circuits for non-POST/PUT methods:

```rust
if matches!(req.method(), &Method::POST | &Method::PUT)
    && has_body(&req)
    && !is_json_content_type(…)
{
    return Err(unsupported_media_type(…));
}
Ok(next.run(req).await)
```

`GET /health` and `GET /admin` pass through untouched in practice.

### Why refactor then?

1. **Intent is obscured.** A reader seeing a global `.layer(…)` has to
   understand the middleware's internal guard to know that `/health` is safe.
   Scoped application makes the intent explicit without reading the
   implementation.

2. **Future-proofing.** A new GET endpoint that returns a binary blob, an
   event-stream, or an HTML page would silently inherit the layer. With scoped
   application, only endpoints added to the `api_routes` group are affected.

3. **Testability.** Scoped routers make it easier to unit-test each group in
   isolation.

---

## Proposed Change (requires `src/lib.rs` write access)

Split the router into two groups and apply the middleware only to the JSON API
group:

```rust
pub fn router(state: AppState) -> axum::Router {
    // Routes that accept a JSON body and must have the content-type enforced.
    let api_routes = axum::Router::new()
        .route("/signup",       axum::routing::post(api::auth::signup))
        .route("/login",        axum::routing::post(api::auth::login))
        .route("/verify-otp",   axum::routing::post(api::auth::verify_otp))
        .route("/logout",       axum::routing::post(api::auth::logout))
        .route("/me",           axum::routing::get(api::me::get))
        .route("/wallet/create",axum::routing::post(api::wallets::create))
        .route("/wallet",       axum::routing::get(api::wallets::get))
        .route("/balance",      axum::routing::get(api::balances::get))
        .route("/transactions", axum::routing::get(api::transactions::list))
        .route("/withdraw",     axum::routing::post(api::withdrawals::create))
        .route("/withdrawals",  axum::routing::get(api::withdrawals::list))
        .route("/payment-requests",
            axum::routing::post(api::payment_requests::create)
                .get(api::payment_requests::list),
        )
        .route("/payment-requests/{id}",
            axum::routing::get(api::payment_requests::get),
        )
        .route("/webhooks/termii", axum::routing::post(api::webhooks::termii))
        // Apply the middleware ONLY to this group of routes.
        .layer(axum::middleware::from_fn(middleware::require_json_content_type));

    // Routes explicitly exempt from the JSON content-type middleware:
    // - /health is a bare liveness probe (no body, no JSON)
    // - /admin serves an HTML dashboard (not JSON)
    // - /admin/* are API routes but serve JSON only when authenticated, and
    //   all use GET — the middleware would short-circuit anyway, but being
    //   explicit here is safer.
    let non_json_routes = axum::Router::new()
        .route("/",
            axum::routing::get(|| async { "aframp" }),
        )
        .route("/health",
            // Exempt: liveness probe, never has a body, never expects JSON.
            axum::routing::get(|| async { axum::http::StatusCode::NO_CONTENT }),
        )
        .route("/admin",       axum::routing::get(api::admin::dashboard))
        .route("/admin/overview", axum::routing::get(api::admin::overview))
        .route("/admin/merchants", axum::routing::get(api::admin::merchants))
        .route("/admin/users",  axum::routing::get(api::admin::users))
        .route("/admin/wallets", axum::routing::get(api::admin::wallets))
        .route("/admin/transactions", axum::routing::get(api::admin::transactions))
        .route("/admin/withdrawals", axum::routing::get(api::admin::withdrawals))
        .route("/admin/payment-requests", axum::routing::get(api::admin::payment_requests));

    axum::Router::new()
        .merge(api_routes)
        .merge(non_json_routes)
        .with_state(state)
}
```

### Comment to add in `src/lib.rs`

```rust
// `require_json_content_type` is applied only to the `api_routes` group.
// Routes that never receive a JSON body (/health, /admin, /admin/*) are
// placed in `non_json_routes` and are deliberately exempt — the middleware
// would short-circuit for GET requests anyway, but explicit scoping makes
// the intent clear and prevents future GET routes from accidentally
// inheriting the layer.
```

---

## Test Coverage

`tests/middleware_scope.rs` (added alongside this doc) verifies:

| Test | What it checks |
|------|---------------|
| `health_returns_204_without_content_type_header` | `GET /health` → `204` with no `Content-Type` |
| `health_returns_204_with_wrong_content_type` | `GET /health` with `text/plain` → `204` (not 415) |
| `health_is_never_415` | `GET /health` must never return `415` |
| `admin_dashboard_is_not_415` | `GET /admin` → not `415` |
| `admin_with_html_content_type_is_not_415` | `GET /admin` with `text/html` → not `415` |
| `post_with_wrong_content_type_still_returns_415` | `POST /signup` with `text/plain` body → `415` (middleware still fires for API routes) |
| `middleware_scope_design_note` | Inline documentation; always passes |

These tests pass today (the current behaviour is correct) and will continue
passing after the refactor — they are a regression guard, not a TDD scaffold.

---

## References

- [`src/lib.rs`](../src/lib.rs) — `router()` function
- [`src/middleware.rs`](../src/middleware.rs) — `require_json_content_type`
- [Axum nested router docs](https://docs.rs/axum/latest/axum/struct.Router.html#method.nest)
- [Issue #1275](https://github.com/kellymusk/Aframp-backend/issues/1275)
