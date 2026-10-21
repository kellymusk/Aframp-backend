# Security Policy

Aframp handles financial transactions and stores encrypted Stellar wallet keys. We take security seriously and welcome responsible disclosure of vulnerabilities.

## Supported Versions

Security fixes are applied to the following branches:

| Branch | Supported |
|--------|-----------|
| `main` | :white_check_mark: Yes |
| `dev-backend` | :white_check_mark: Yes |
| All other branches / forks | :x: No |

Only the latest commit on the supported branches receives security patches. If you are running a fork or an older commit, please rebase onto `main` or `dev-backend` before reporting an issue that may already be fixed.

## Reporting a Vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Instead, use one of the following private channels:

1. **GitHub Private Vulnerability Reporting (preferred)** — go to the repository's **Security** tab and click **"Report a vulnerability"**. This opens a private advisory visible only to maintainers.
2. **Email** — send details to **security@aframp.io**. If you wish, encrypt your report using our PGP key (available on request).

Please include as much of the following as possible:

- A description of the vulnerability and its impact
- Steps to reproduce (proof-of-concept, request/response, or test case)
- Affected branch, commit hash, and environment
- Any suggested remediation

## Response Time Commitment

- **Acknowledgment:** within **3 business days** of your report.
- **Initial assessment / triage:** within **7 business days**.
- **Fix or mitigation plan:** communicated as part of triage, prioritized by severity.

We will keep you informed of progress and coordinate a disclosure timeline with you. We ask that you give us reasonable time to remediate before any public disclosure.

## Scope

The following are **in scope** for security reports:

- Authentication and authorization flaws (including OTP flows)
- Exposure or mishandling of encrypted Stellar wallet keys or other secrets
- Payment request / transaction integrity issues (e.g. amount or recipient tampering)
- Injection vulnerabilities (SQL, command, etc.)
- Sensitive data exposure via API responses or logs
- Server-side request forgery, broken access control, and privilege escalation

## Out of Scope

The following are generally **out of scope**:

- Vulnerabilities in test accounts, testnet-only deployments, or example harnesses
- Rate-limit bypasses that do not affect funds, keys, or account integrity
- Denial-of-service via volumetric traffic or resource exhaustion without a concrete impact
- Missing security headers or best-practice hardening without a demonstrated exploit
- Social engineering, physical attacks, or spam
- Reports generated solely by automated scanners without a reproducible impact

If you are unsure whether something is in scope, report it privately and we will assess it.

## Bug Bounty

We do not currently operate a formal paid bug bounty program. We deeply appreciate responsible disclosures and will credit reporters (with permission) in the advisory once a fix is released.

Thank you for helping keep Aframp and its users safe.
