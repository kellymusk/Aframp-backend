# Security: send `Strict-Transport-Security` from the app

**Issue:** [#1248](https://github.com/kellymusk/Aframp-backend/issues/1248) / [#1488](https://github.com/kellymusk/Aframp-backend/issues/1488)
**Component:** `src/middleware.rs`, `src/main.rs`, `tests/middleware_scope.rs`, `DEPLOY.md`, `SECURITY.md`

## Problem

HSTS is only set by the sample Caddy config. Deployed without that proxy, the
app never tells browsers to stick to HTTPS. Without HSTS, browsers are not
instructed to enforce HTTPS for future requests, leaving a window for
downgrade attacks.

## Implemented design

Rather than pulling in the `set-header` feature of `tower-http`, the header is
set by a small middleware in `src/middleware.rs`. This keeps the dependency
surface unchanged and lets the middleware share the same production-detection
logic used elsewhere in the service.

The middleware:

- Reads the production trigger from the application config. The service is
  considered to be running in production when `APP_ENV=production` (which also
  drives `cookie_secure`; `COOKIE_SECURE=true` is the default in production).
- When in production, adds
  `Strict-Transport-Security: max-age=31536000; includeSubDomains` to every
  response.
- Uses `if_not_present` semantics: if a response already carries a
  `Strict-Transport-Security` header (for example, set by a TLS-terminating
  reverse proxy such as Caddy or nginx), the middleware leaves it untouched so
  the proxy's value wins.
- When not in production (plain-HTTP dev, `COOKIE_SECURE=false`), no header is
  added, so a dev browser is never pinned to HTTPS for the host.

The middleware is applied in `src/main.rs` alongside the existing CORS and
other layers, so it runs for all API responses.

### Trigger summary

| Condition | Header sent |
| --- | --- |
| `APP_ENV=production` / `COOKIE_SECURE=true` (default in prod) | `Strict-Transport-Security: max-age=31536000; includeSubDomains` |
| `COOKIE_SECURE=false` (plain-HTTP dev) | none |
| Proxy already set `Strict-Transport-Security` | proxy value preserved (`if_not_present`) |

## Tests

`tests/middleware_scope.rs` builds the layered app the same way
`tests/cors_preflight.rs` does for CORS and asserts:

- `Strict-Transport-Security` is present in responses when
  `APP_ENV=production` / `cookie_secure` is true.
- The header is absent when `cookie_secure` is false.
- A pre-existing `Strict-Transport-Security` header is not overwritten.

## Documentation

- `DEPLOY.md` documents the reverse-proxy configuration for HSTS
  (Caddy/nginx), noting that the app also sets the header in production and
  that the proxy value takes precedence via `if_not_present`.
- `SECURITY.md` lists HSTS under the security checklist.

## Tasks

- [x] Review `docs/proposals/1248-hsts-header.md` and add implementation details.
- [x] Add a test asserting `Strict-Transport-Security` is present in responses when `APP_ENV=production`.
- [x] Update `DEPLOY.md` with proxy configuration for HSTS.
- [x] Add `HSTS` to the security checklist in `SECURITY.md`.