# Test Specification: Strict-Send Amount-Received Tracking (#1502)

## Overview

This document is the test specification for issue **#1502**, an enhancement to track the amount received versus the amount sent for Stellar path payments (strict-send) in payment request reconciliation.

It reviews `docs/proposals/1110-strict-send-amount-received.md` and defines the concrete test cases to be added in `tests/deposit_flow.rs`.

> **Note:** The source changes described here (in `src/blockchain/worker.rs` → `process_deposit()`) must be made by a maintainer. This document and the accompanying tests only specify and verify the expected behavior.

## Background

`docs/proposals/1110-strict-send-amount-received.md` documents the following gap:

- When a customer pays via a Stellar **path payment (strict-send)**, the amount received by the merchant wallet may differ from the amount sent.
- The current `process_deposit()` path records `amount_stroops` as whatever arrived on-chain.
- A payment request for, e.g., 100 cNGN may be matched as "paid" even if the customer used strict-send and the slippage was acceptable.
- The proposal recommends recording both `amount_sent` and `amount_received` to enable accurate reconciliation.

## Scope

This specification covers the reconciliation behavior of `process_deposit()` in `src/blockchain/worker.rs`:

1. Underpayment must result in `mark_partial`, never `mark_paid`, regardless of path payment type.
2. Exact payment and overpayment must result in `mark_paid`.
3. For `path_payment_strict_send`, the distinction between the amount sent and the amount received must be preserved and used for reconciliation.

## Definitions

- **`amount_stroops`**: The amount actually received by the merchant wallet, as observed on-chain (in stroops). This is the value used for reconciliation against the payment request.
- **`amount_sent`**: The amount the customer specified to send (relevant for strict-send path payments).
- **`amount_received`**: The amount that arrived at the merchant wallet after path conversion and slippage.
- **`request_amount`**: The amount specified in the payment request.
- **`mark_paid`**: Reconciliation outcome indicating the request is fully satisfied.
- **`mark_partial`**: Reconciliation outcome indicating the request is only partially satisfied.

## Test Cases

All tests live in `tests/deposit_flow.rs`.

### TC-1: Underpayment triggers `mark_partial` (strict-send)

- **Given** a payment request for `request_amount` = 100 cNGN.
- **And** a deposit arrives via `path_payment_strict_send` with `amount_stroops` (received) less than `request_amount` (e.g., 99 cNGN received due to slippage).
- **When** `process_deposit()` processes the deposit.
- **Then** the payment request must be marked `mark_partial`.
- **And** it must **not** be marked `mark_paid`.

### TC-2: Underpayment triggers `mark_partial` (non-path payment)

- **Given** a payment request for `request_amount` = 100 cNGN.
- **And** a deposit arrives via a standard (non-path) payment with `amount_stroops` less than `request_amount`.
- **When** `process_deposit()` processes the deposit.
- **Then** the payment request must be marked `mark_partial`.
- **And** it must **not** be marked `mark_paid`.

> TC-1 and TC-2 together verify that underpayment results in `mark_partial` **regardless of path payment type**.

### TC-3: Exact payment triggers `mark_paid`

- **Given** a payment request for `request_amount` = 100 cNGN.
- **And** a deposit arrives with `amount_stroops` exactly equal to `request_amount`.
- **When** `process_deposit()` processes the deposit.
- **Then** the payment request must be marked `mark_paid`.

### TC-4: Overpayment triggers `mark_paid`

- **Given** a payment request for `request_amount` = 100 cNGN.
- **And** a deposit arrives with `amount_stroops` greater than `request_amount` (e.g., 101 cNGN).
- **When** `process_deposit()` processes the deposit.
- **Then** the payment request must be marked `mark_paid`.

### TC-5: Sent vs. received distinction for `path_payment_strict_send`

- **Given** a payment request for `request_amount` = 100 cNGN.
- **And** a deposit arrives via `path_payment_strict_send` where `amount_sent` = 100 cNGN but `amount_received` (i.e., `amount_stroops`) = 98 cNGN due to slippage.
- **When** `process_deposit()` processes the deposit.
- **Then** the reconciliation must use `amount_received` (`amount_stroops`), not `amount_sent`.
- **And** the payment request must be marked `mark_partial` (since `amount_received` < `request_amount`).
- **And** both `amount_sent` and `amount_received` must be recorded for reconciliation purposes.

### TC-6: Strict-send with sufficient received amount triggers `mark_paid`

- **Given** a payment request for `request_amount` = 100 cNGN.
- **And** a deposit arrives via `path_payment_strict_send` where `amount_sent` = 105 cNGN and `amount_received` (`amount_stroops`) = 100 cNGN (or greater).
- **When** `process_deposit()` processes the deposit.
- **Then** the payment request must be marked `mark_paid`.
- **And** both `amount_sent` and `amount_received` must be recorded.

## Expected Behavior Summary

| Test | Path Type | `amount_sent` | `amount_received` (`amount_stroops`) | `request_amount` | Expected Outcome |
|------|-----------|---------------|--------------------------------------|------------------|------------------|
| TC-1 | `path_payment_strict_send` | 100 | 99 | 100 | `mark_partial` |
| TC-2 | standard | — | 99 | 100 | `mark_partial` |
| TC-3 | standard | — | 100 | 100 | `mark_paid` |
| TC-4 | standard | — | 101 | 100 | `mark_paid` |
| TC-5 | `path_payment_strict_send` | 100 | 98 | 100 | `mark_partial` |
| TC-6 | `path_payment_strict_send` | 105 | 100 | 100 | `mark_paid` |

## Implementation Notes

- The reconciliation logic lives in `src/blockchain/worker.rs` → `process_deposit()`.
- Reconciliation must compare `amount_stroops` (the received amount) against the payment request amount.
- For `path_payment_strict_send`, both `amount_sent` and `amount_received` should be recorded to enable accurate reconciliation.
- **The worker/source change is for a maintainer.** The tests in `tests/deposit_flow.rs` are written against the expected behavior described above and will pass once the maintainer implements the source change.

## References

- `docs/proposals/1110-strict-send-amount-received.md`
- `src/blockchain/worker.rs` → `process_deposit()`
- `tests/deposit_flow.rs`