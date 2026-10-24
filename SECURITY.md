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
Aframp handles real Stellar secret seeds, Paystack API keys, and Nigerian customer and merchant financial data. We take reports of vulnerabilities seriously and appreciate researchers who disclose them responsibly.

## Supported versions

Aframp is in active MVP development and has no tagged releases yet. Only the latest code on the default branch receives security fixes.

| Version | Supported |
|---|---|
| `0.1.x` (latest `master`) | Yes |
| Older commits and forks | No |

## Personal information in application logs

Application logs must not contain raw personal information or complete
third-party response payloads. In particular, do not log merchant or customer
email addresses, phone numbers, names, account numbers, or payment-provider
response bodies. Use stable error codes and opaque entity identifiers when
diagnostic context is needed. Redact or omit sensitive values before passing
them to tracing fields; production log scrubbing is defense in depth, not a
replacement for safe logging at the source.

## Admin dashboard response headers

The `/admin` HTML response must include a Content-Security-Policy that limits
resources to the application origin, disables plugins with `object-src 'none'`,
and prevents framing with `frame-ancestors 'none'`. Any inline scripts must use
a cryptographically random nonce generated for that response, and the same nonce
must appear in the CSP and each permitted script element. Do not use
`'unsafe-inline'` in `script-src`. The response must also include
`X-Frame-Options: DENY` and `X-Content-Type-Options: nosniff`.

## Reporting a vulnerability

**Do not open a public issue, pull request, or discussion for a security problem.**

Report it privately through GitHub's private vulnerability reporting:

1. Go to the repository's **Security** tab.
2. Click **Report a vulnerability** (or open [a new advisory](https://github.com/kellymusk/Aframp-backend/security/advisories/new) directly).
3. Fill in the advisory form.

Please include:

- The affected endpoint, file, or component
- Steps to reproduce, or a proof of concept
- The impact you believe it has (for example: fund loss, key exposure, account takeover, data leak)
- Any suggested fix, if you have one

Never include real secret seeds, API keys, or personal data belonging to anyone other than yourself in a report. Use testnet accounts and test keys wherever possible.

## What to expect

| Stage | Timeline |
|---|---|
| Acknowledgement of your report | Within 48 hours |
| Initial assessment and severity rating | Within 7 days |
| Fix developed and released | Within 90 days of the report |
| Public disclosure | After the fix ships, or 90 days after the report, whichever comes first |

We will keep you updated as the report moves through these stages. If a fix needs longer than 90 days, we will agree a revised disclosure date with you before that deadline passes. With your permission, we will credit you in the published advisory.

## Scope

In scope:

- This backend (`src/`, `migrations/`, `cloudflare/`, and deployment config in this repository)
- Handling of wallet secret seeds, JWTs, OTPs, and Paystack/Termii webhooks

Out of scope:

- Vulnerabilities in third-party services (Stellar, Horizon, Paystack, Termii, Cloudflare); report those to the vendor
- Denial-of-service through volumetric traffic
- Findings that require a compromised host or leaked production credentials

## Safe harbor

We will not pursue legal action against researchers who act in good faith under this policy: who avoid privacy violations, data destruction, and service disruption, only access data they need to demonstrate the issue, and give us reasonable time to fix it before any public disclosure.
