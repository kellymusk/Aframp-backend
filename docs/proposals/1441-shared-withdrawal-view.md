# Proposal: shared withdrawal view layer (#1441)

## Problem
`src/api/withdrawals.rs` (`WithdrawalView` + `to_view`) and `src/api/admin.rs`
(`AdminWithdrawalRow`) map the same `Withdrawal` model independently. New model
fields must be added twice, and the views have already diverged.

## Current differences
| Field | Merchant view | Admin view | Should diverge? |
|---|---|---|---|
| `idempotency_key` | no | yes | No — merchants supplied it; expose to both |
| `merchant_id` | no (implicit) | yes | Yes — admin-only context |
| Core fields (id, amount, status, destination, timestamps) | yes | yes | No |

## Proposal
- Single `WithdrawalView` built via `From<&Withdrawal>` in a shared module
  (e.g. `src/api/views.rs`).
- `AdminWithdrawalRow` embeds it with `#[serde(flatten)]` and adds admin-only
  fields (`merchant_id`).
- Adding a model field then needs one change.

## Tests
A JSON diff test should assert both views serialize every `Withdrawal` field
(minus an explicit allow-list of intentionally hidden fields).
