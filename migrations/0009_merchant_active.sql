-- Add active flag to merchants to support admin suspension (#1316)
ALTER TABLE merchants
    ADD COLUMN active BOOLEAN NOT NULL DEFAULT true;
