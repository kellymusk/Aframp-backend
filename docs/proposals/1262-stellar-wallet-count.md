# Proposal: #1262 — count Stellar wallets in admin overview

> **Blocked for contributors:** this PR does not change the overview behavior.
> The query and response model live under `src/`, a protected path on
> `dev-backend` per `CONTRIBUTING.md`.

## Maintainer implementation checklist

- Filter `total_wallets` with `WHERE network = 'stellar'` so the overview agrees
  with the deposit worker's wallet selection.
- Keep the existing `AdminOverview` response shape and document the clarified
  count in `API.md`.
- Add or extend an admin overview integration test with both Stellar and
  non-Stellar wallets; assert only Stellar wallets contribute to the count.

This proposal is a handoff only; it does not close or resolve #1262.