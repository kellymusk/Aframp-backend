# Proposal: #1261 — single-withdrawal lookup endpoint

> **Blocked for contributors:** this PR does not implement the endpoint. The
> required router, handler, and data-access changes are under `src/`, which
> `CONTRIBUTING.md` marks as protected on `dev-backend`.

## Maintainer implementation checklist

- Add authenticated `GET /withdrawals/{id}` routing and handler.
- Load the withdrawal by UUID and enforce merchant ownership before returning
  its view. Follow the issue's requested `403` response for another merchant;
  return `404` when the id does not exist.
- Add integration coverage for owned, missing, and cross-merchant ids.
- Update `API.md` and `openapi.yaml` with the route and error contract.

This proposal is a handoff only; it does not close or resolve #1261.