# Security Policy

Aframp handles real Stellar secret seeds, Paystack API keys, and Nigerian customer and merchant financial data. We take reports of vulnerabilities seriously and appreciate researchers who disclose them responsibly.

## Supported versions

Aframp is in active MVP development and has no tagged releases yet. Only the latest code on the default branch receives security fixes.

| Version | Supported |
|---|---|
| `0.1.x` (latest `master`) | Yes |
| Older commits and forks | No |

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
