# Proposal: `confirmed_at` on payments (#1437)

## Problem
`updated_at` also moves on flags and admin actions, so merchants can't reliably tell when a deposit was confirmed.

## Change
1. Migration: `ALTER TABLE payments ADD COLUMN confirmed_at timestamptz;` (nullable; existing rows stay null, optionally backfilled from `updated_at` where `status = 'confirmed'`).
2. `promote_confirmed()` in `src/services/payments.rs` sets `confirmed_at = now()` in the same `UPDATE` that sets `status = 'confirmed'`, only if it is still null.
3. `src/api/transactions.rs` exposes `confirmed_at` (null until confirmed). Already added to `openapi.yaml`.
4. Add the migration to the migration index.

## Test
After a payment is promoted, `GET /transactions` returns a non-null `confirmed_at`; later admin flags change `updated_at` but not `confirmed_at`.
