# Architecture Decision Records

This directory captures the key design decisions made during Aframp backend
development, along with the context and trade-offs that shaped each choice.
ADRs are append-only — a superseded decision gets a new ADR marked
"Supersedes ADR-NNN", not an edit to the original.

| ADR | Title | Status |
|-----|-------|--------|
| [ADR-001](ADR-001-custodial-wallet-design.md) | Custodial per-merchant wallet design | Accepted |
| [ADR-002](ADR-002-otp-gated-signup.md) | OTP-gated signup and login (phone-number 2FA) | Accepted |
| [ADR-003](ADR-003-hmac-otp-storage.md) | HMAC-based OTP code storage (not bcrypt/Argon2) | Accepted |
| [ADR-004](ADR-004-commit-before-paystack.md) | Commit-before-Paystack withdrawal design | Accepted |
