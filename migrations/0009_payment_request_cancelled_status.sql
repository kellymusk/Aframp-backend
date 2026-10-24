-- Migration: 0009_payment_request_cancelled_status.sql
-- Description: Extend the payment_requests.status CHECK constraint to include 'cancelled'
-- so merchants can cancel pending payment requests (issue #1201).

-- Drop the existing CHECK constraint on status (if present) and recreate it
-- with the additional 'cancelled' value.

DO $$
DECLARE
    constraint_name text;
BEGIN
    SELECT con.conname
    INTO constraint_name
    FROM pg_constraint con
    JOIN pg_class rel ON rel.oid = con.conrelid
    JOIN pg_namespace nsp ON nsp.oid = rel.relnamespace
    WHERE rel.relname = 'payment_requests'
      AND con.contype = 'c'
      AND pg_get_constraintdef(con.oid) ILIKE '%status%';

    IF constraint_name IS NOT NULL THEN
        EXECUTE format('ALTER TABLE payment_requests DROP CONSTRAINT %I', constraint_name);
    END IF;
END $$;

ALTER TABLE payment_requests
    ADD CONSTRAINT payment_requests_status_check
    CHECK (status IN ('pending', 'paid', 'partial', 'expired', 'cancelled'));