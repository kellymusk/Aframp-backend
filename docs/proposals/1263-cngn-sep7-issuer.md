# Proposal: #1263 — configure the cNGN SEP-7 issuer

> **Blocked for contributors:** this PR does not generate cNGN payment URIs.
> Configuration, URI construction, and startup behavior are in protected
> `src/` paths on `dev-backend` per `CONTRIBUTING.md`.

## Maintainer implementation checklist

- Confirm the requested environment-variable spelling before implementation:
  the issue says `CNGGN_ISSUER_ADDRESS` (with two consecutive `G`s), while the
  asset is `cNGN`.
- Add the issuer setting to application configuration and generate SEP-7 URIs
  only when a trusted issuer address is configured; keep `sep7_uri` null when
  it is unavailable.
- Warn at startup when the setting is absent, without logging secrets.
- Document the setting and URI behavior in `.env.example`, `API.md`, and
  `openapi.yaml`; add an integration test for configured and unconfigured cases.

This proposal is a handoff only; it does not close or resolve #1263.