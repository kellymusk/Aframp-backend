# ADR-004: Commit-before-Paystack withdrawal design

**Status:** Accepted  
**Date:** 2024-11-01  
**Deciders:** Core team

---

## Context

A withdrawal request involves two side effects that cannot be made atomic with
each other:

1. A local database write (debit the merchant's available balance, insert a
   `withdrawals` row).
2. An outbound HTTP call to Paystack's Transfers API (resolve bank account,
   create recipient, initiate transfer).

Three sequencing patterns were considered:

**Option A — Call Paystack first, then commit**  
Initiate the Paystack transfer, and only if it succeeds debit the balance and
create the withdrawal row.

**Option B — Wrap both in a single transaction (impossible)**  
Hold a database transaction open across the HTTP call to Paystack. This cannot
work: a long-held transaction blocks Postgres rows, and the HTTP call may
time out or hang indefinitely — effectively a table-level lock for the
duration of an external API call.

**Option C — Commit locally first, then call Paystack**  
Debit the balance and insert a `pending` withdrawal row in a committed
transaction *before* calling Paystack. If Paystack fails, issue a compensating
transaction that refunds the balance and marks the row `failed`.

---

## Decision

**Option C (commit-before-Paystack) was chosen.**

The implementation is in `src/services/withdrawals.rs`. The reasoning is stated
in an inline comment there:

> *Commit the debit + pending row before ever calling out to Paystack. This
> guarantees a durable record that the withdrawal was attempted regardless of
> what happens next — nothing about the external call can make this local state
> vanish.*

The failure path creates a compensating transaction:

> *Refund + mark failed as one atomic unit, in a fresh transaction — the
> original debit is already committed, so this is a compensating action, not a
> rollback. Keeps an audit trail instead of pretending the attempt never
> happened.*

And if the post-Paystack write itself fails:

> *If this write fails, the row is left `pending` with no provider info —
> recoverable later, and safe: it under-states what happened (a real transfer
> may have gone out) rather than erasing the record that a withdrawal was
> attempted at all.*

---

## Consequences

**Positive:**

- A durable audit trail exists for every withdrawal attempt, regardless of
  whether Paystack succeeded, failed, timed out, or returned an ambiguous
  response.
- The merchant's balance is always consistent: either debited (withdrawal
  attempted) or refunded (attempt definitively failed). There is no code path
  that silently loses the ledger record.
- The approach handles the three real failure modes tested live against
  Paystack: bad bank account, amount below minimum, insufficient platform
  balance — each produces a `failed` row with a `failure_reason`, never a
  silent loss.

**Negative / open items:**

- **Phantom debit window:** between the initial commit and the completion of the
  Paystack call (or its compensating refund), the merchant sees a reduced
  `available` balance even if the transfer ultimately fails. This window is
  bounded by Paystack's API response time (typically < 5 s) and is considered
  acceptable for the MVP.
- **Ambiguous `pending` rows:** if the service crashes after the commit but
  before the Paystack call completes, the row is left `pending` with no
  `provider_reference`. A background reconciliation job should be written to
  query Paystack for the status of any `pending` rows older than a threshold
  and resolve them. This does not yet exist.
- **Option A** would avoid the phantom debit window but creates the opposite,
  worse risk: Paystack initiates a real bank transfer and then the local write
  fails — money has left the platform with no record.
