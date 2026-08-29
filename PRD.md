# Aframp — Product Requirements Document

**Status:** Living document · **Last updated:** 2026-08-10 · **Scope:** Aframp Pay backend (this repository)

This PRD exists because the backend has now reached a point where we have real, verified clarity on what works — not just what was planned. Every status claim below reflects something exercised end-to-end (real Postgres writes, a real Stellar testnet transaction), not a reading of the code. Where we don't yet have an answer, that's marked as an open decision rather than papered over.

---

## 1. Overview

Aframp is a merchant point-of-sale acceptance network for Stellar-powered payments, starting in Nigeria. The insight: Nigerians already understand POS terminals and bank-transfer payments ("give me your account number," "use the POS"). Aframp adds **scan and pay** as a familiar-feeling option, settled on Stellar, without merchants or customers needing to understand blockchain concepts.

```
Customer → Scan → Pay → Confirmed
```

From the merchant's point of view, that's the whole product. Stellar is the settlement layer underneath; Aframp is the experience on top.

**Planned product layers** (long-term — this repo is the backend under the first one):

| Layer | Purpose |
|---|---|
| Aframp Pay | Merchant-facing: payment requests, QR codes, receive Stellar payments, receipts, revenue tracking |
| Aframp Wallet | Consumer wallet built for spending, not just holding |
| Aframp Business | Merchant dashboard: analytics, reconciliation, multi-location, invoices |
| Aframp API | Infrastructure layer for other African fintechs to integrate Stellar payments |

## 2. Problem

Cross-border commerce in Africa is fragmented across national payment systems — multiple currencies, high fees, slow settlement, and merchant onboarding that doesn't travel across borders. Stablecoins and blockchain rails already move value globally, fast and cheap. What's missing is the everyday, physical-commerce bridge between the two.

Aframp's bet: onboard merchants first with a familiar payment experience, and let consumer demand for Stellar wallets follow — merchant acceptance drives adoption, not the other way around.

## 3. Users

| User | Role in MVP | Notes |
|---|---|---|
| **Merchant** | Primary user of this backend | Signs up, receives a Stellar wallet Aframp custodies on their behalf, watches balance grow as customers pay, withdraws to a bank account |
| **Customer** | Pays a merchant | Not a user of this backend directly — assumed to bring any Stellar-compatible wallet. No Aframp Wallet app exists yet (post-MVP) |
| **Aframp (platform)** | Custodian + settlement operator | Holds encrypted keys for every merchant wallet; will eventually sweep funds to a platform settlement wallet (not built yet — see [§9](#9-open-decisions)) |

## 4. Goals — MVP scope

The MVP is scoped to exactly seven capabilities, in the order they occur in the payment flow. Status reflects what's been verified in this repository as of the date above.

| # | Capability | Requirement | Status |
|---|---|---|---|
| 1 | Merchant account | A merchant can create an account and authenticate | ✅ **Done** — signup/login, Argon2-hashed passwords, JWT sessions. Every subsequent endpoint requires a valid token; verified live that no-token and garbage-token requests both return `401`. |
| 2 | Payment request generation | A merchant can generate a request for a specific amount | ✅ **Done** — `POST /payment-requests` creates a request tied to the merchant's wallet, with a unique correlation memo and a configurable expiry (15min default). `GET /payment-requests/{id}` is deliberately public (no auth) so a customer's wallet can read it before paying. |
| 3 | QR-based payment | A customer can scan a QR code to pay | ✅ **Done for XLM** — the request response includes a SEP-0007 `web+stellar:pay` URI any Stellar wallet can open directly. Verified live: correct destination, correct amount conversion (25,000,000 stroops → `2.5000000` in the URI). **Not yet for cNGN** — no real cNGN issuer address is configured (see [§9.3](#9-open-decisions)), so a credit-asset request returns `sep7_uri: null` rather than a guessed/wrong issuer. Detection also now correlates a specific incoming payment to its request via Stellar memo, not just "something arrived." |
| 4 | Stellar transaction creation | The system can create a Stellar-native receiving address | ✅ **Done, narrower than the name implies** — `/wallet/create` generates a real ed25519 keypair, encoded as a genuine Stellar `G...` address. Verified against Horizon directly. The platform does **not** yet build or sign outbound Stellar transactions anywhere (no XDR construction, no submission) — this row covers wallet/address creation, not general transaction authoring. |
| 5 | Transaction monitoring | The system detects incoming Stellar payments | ✅ **Done** — a background worker polls Horizon per merchant wallet on a timer, handling both `create_account` (how a brand-new wallet is always first funded on Stellar) and `payment`/`path_payment` ops. Proven with a real testnet transaction: funded a generated wallet via Stellar's friendbot and watched the exact transaction hash and amount land in the system within one poll cycle. |
| 6 | Payment confirmation | A detected payment is confirmed back to the merchant | 🚧 **Working, but simplified** — detected deposits move `detected → verified → confirmed` and update the merchant's balance immediately. There is no real "wait N ledger confirmations" threshold yet (open TODO in `blockchain/worker.rs`). |
| 7 | Merchant transaction history | A merchant can see their payment history | ✅ **Done** — `GET /transactions` returns real detected payments, paginated. |

**Also implemented, one layer past the original MVP list:** fiat withdrawal requests. `POST /withdraw` atomically debits a merchant's available balance and records a withdrawal in the same DB transaction, with insufficient-balance and validation checks enforced server-side. **However:** no money actually leaves the system yet — there's no real payout provider wired in (see [§9](#9-open-decisions)). This is ledger accounting, not a working cash-out.

## 5. Non-goals for MVP

Explicitly out of scope right now (per founder direction, not an oversight):

- KYC/AML and compliance depth
- Multi-region / multi-currency infrastructure beyond cNGN/XLM on Stellar testnet
- Analytics dashboards (Aframp Business layer)
- Physical POS hardware
- Cross-border payment corridors
- Public-facing API for third-party fintechs (Aframp API layer)
- A consumer-facing Aframp Wallet app

## 6. Core user flows

**Merchant onboarding**
1. Merchant signs up (`POST /signup`) → gets a JWT session.
2. Merchant creates a wallet (`POST /wallet/create`) → Aframp generates a real Stellar keypair, returns the public address, encrypts and stores the private key server-side (custodial model — not "bring your own wallet").

**Payment (deposit) flow — as it works today**
1. Merchant creates a payment request for an amount (`POST /payment-requests`) → gets back a destination address, a correlation memo, and (for XLM) a scannable SEP-0007 URI.
2. Customer opens the request (`GET /payment-requests/{id}`, no auth needed) or scans the QR, and pays with any Stellar-compatible wallet, including the memo.
3. The background worker's next poll cycle (default every 60s, configurable) detects the payment via Horizon — now fetched with the parent transaction joined in, so the memo comes back with it.
4. The payment is recorded, moved through `detected → verified → confirmed`, and the merchant's balance updates. If the memo matches a pending request, that request is marked `paid` and linked to the real payment — not just a raw balance change.
5. Merchant sees the confirmed payment in `GET /balance` and `GET /transactions`; anyone with the request ID sees it flip to `paid` in `GET /payment-requests/{id}`.

A payment can still land without going through a request at all (manually, or via friendbot on testnet) — it's recorded and confirmed exactly as before, it just has nothing to correlate against, which is unchanged, additive behavior.

**Withdrawal flow — as it works today**
1. Merchant requests a withdrawal (`POST /withdraw`) with a bank account.
2. Available balance is debited immediately; a `pending` withdrawal record is created.
3. **Nothing pays out.** This is the biggest gap between "the ledger says X" and "the merchant has X in their bank account." See [§9](#9-open-decisions).

## 7. System design (current)

- **Custody model:** custodial. Aframp generates and holds every merchant's Stellar private key, AES-256-GCM encrypted at rest (`WALLET_ENCRYPTION_KEY`). Merchants never see or manage a seed phrase.
- **Deposit detection:** poll-based, not event-streamed. A background worker queries Horizon's per-account payments feed for every wallet in the database on a fixed interval. This is simple and has been proven correct, but doesn't scale indefinitely — see [§9](#9-open-decisions).
- **Ledger:** Postgres is the source of truth for merchant-facing balance, not the Stellar ledger directly. Balance changes only ever originate from a detected, successful Stellar operation.
- **Auth:** every endpoint that touches merchant data or money requires a valid JWT, enforced structurally via an Axum extractor (the handler cannot compile without it) — not a per-handler check that could be forgotten.
- **What's explicitly *not* built:** outbound Stellar transaction construction/signing (Aframp never signs a payment itself — customers always pay from their own wallet), a settlement/sweep wallet, any real payout provider funding (Stage A), a real cNGN issuer address (blocks cNGN QR codes specifically).

## 8. Current status ledger

A condensed view of what's verified vs. still a stub, independent of the MVP numbering above — see the repository [`README.md`](README.md) for the full version with proof points.

**✅ Verified working today:** merchant signup/login, universal auth enforcement, real Stellar wallet generation, encrypted key custody, real Stellar deposit detection (proven with an actual testnet transaction), balance ledger, transaction history, payment request generation + SEP-0007 QR for XLM, memo-based payment-to-request correlation, withdrawal request + ledger accounting, Paystack Transfers wired into `/withdraw` and live-tested against three distinct real failure modes.

**🚧 Genuine gaps:** real payout funding (Stage A — no money can currently leave the system, see §9.1), real cNGN issuer address (blocks cNGN QR codes and cNGN-denominated payment requests from being scannable), confirmation-depth threshold (currently immediate), settlement/sweep wallet, and one piece of vestigial dead code (`src/stellar/mod.rs`, from an abandoned memo-based design) still sitting in the tree as cleanup debt.

## 9. Open decisions

These are real product/engineering decisions that need an answer before the next phase of work, not implementation details we can quietly decide alone:

1. **Settlement & payout pipeline.** Originally tracked as two separate questions — a payout provider, and whether to sweep merchant wallets — research into cNGN's own redemption mechanics showed these are actually one two-stage pipeline, not independent choices:
   - **Stage A — crypto → fiat conversion.** Merchant Stellar wallets get swept into an Aframp-controlled platform wallet, which periodically redeems cNGN for real NGN via the cNGN issuer (WrappedCBDC, `docs.cngn.co`). This is the piece the issuer directly solves — but it only pays out to a small set of **pre-whitelisted** Aframp-owned bank accounts, and whitelisting a new destination carries a **24-hour timelock**. That rules it out as a same-day, per-merchant payout mechanism on its own.
   - **Stage B — fiat → individual merchant payout.** Once Aframp holds real NGN, a last-mile rail fans it out to each merchant's own bank account on demand. Paystack Transfers fits this specifically (resolve account → create recipient → initiate transfer → OTP finalize in live mode), as would Flutterwave or a direct NIBSS integration. Paystack requires a **Registered** (not Starter) business with compliance docs submitted before Transfers unlock at all — real lead time worth starting early, independent of when the code gets written.

   Still open within this: which last-mile provider (compare fees and payout speed across Paystack/Flutterwave), and how Stage A redemption is triggered (batched on a schedule, or fired once a platform-wallet balance threshold is crossed).

   **Stage B is now wired and live-tested against Paystack's real test-mode API** (`services/withdrawals.rs` calls a real `PaystackProvider`, not a stub). That testing produced empirical, not theoretical, confirmation of the Stage A dependency: with a real bank account and a valid amount, the transfer still failed with *"Your balance is not enough to fulfil this request"* — Paystack's `source: "balance"` transfers draw from NGN Paystack already holds for Aframp, and there is currently no way to get money into that balance. Paystack's own docs state settlements aren't processed in test mode at all, so this can't be worked around from the API side — Stage A (or an equivalent real funding source) is a hard prerequisite for Stage B to ever return a real `success`, not just a nice-to-have ordering.
2. **Confirmation-depth policy.** Stellar has fast finality (~5s), so "wait N confirmations" matters less than on Bitcoin — but "immediate" still means zero protection against a reorg edge case. Needs an explicit policy, even if the answer is "confirm immediately is fine, here's why."
3. ~~**Payment request / QR payload format.**~~ **Resolved:** SEP-0007 `web+stellar:pay` URIs, implemented and live-verified for XLM. Still genuinely open underneath this: cNGN requests can't get a real QR until there's a real cNGN issuer Stellar address to put in the URI — a guessed one would silently misdirect a customer's payment, so `sep7_uri` is `null` for cNGN today rather than wrong. This is really the same missing piece as the asset-scope question below.
4. **Asset scope, including a real cNGN issuer address.** Today deposit detection recognizes any asset Horizon reports (native XLM or credit assets by code), and payment requests default to XLM specifically because it's what's actually scannable right now. Is cNGN the only supported asset at launch, or does XLM stay first-class? Whichever way that's answered, a real cNGN issuer Stellar address needs to be sourced before cNGN payment requests, QR codes, or deposit handling can be considered complete rather than partially stubbed.
5. **Poll-based detection scaling.** Works today with a handful of wallets. At what wallet count does polling-per-address on a fixed interval become a bottleneck, and is the answer a smarter cursor/backoff strategy, or a move to Horizon's streaming (SSE) API?

## 10. Success metrics (proposed — pending confirmation)

No target numbers exist yet from the business side; these are proposed instrumentation points worth deciding on, not committed targets:

- Time from signup to first wallet created
- Time from a real deposit landing on-chain to it appearing in `/balance` (currently bounded by `STELLAR_POLL_INTERVAL_SECS`, default 60s)
- Withdrawal request → real payout completion time (once a provider is wired in)
- Signup → wallet → first deposit → first withdrawal funnel completion rate

## 11. Risks

- **Custodial key risk.** Aframp holds every merchant's private key. `WALLET_ENCRYPTION_KEY` compromise would expose every merchant's funds. Key management (rotation, HSM/KMS vs. env var) needs to mature before real merchant funds are at stake.
- **No real payout yet.** If withdrawal UX ships before a real payout provider is wired in, merchants would see a debited balance with no way to actually receive funds — a trust-destroying gap if exposed prematurely.
- **Poll-interval latency.** A 60-second default poll interval means a merchant could wait up to a minute to see a payment confirmed — worth deciding if that's acceptable for a POS "instant" feel, or needs tightening.
- **Regulatory scope creep.** Custodying every merchant's private key, converting crypto to fiat through a platform-controlled treasury, and fanning fiat back out to individual merchant bank accounts is, functionally, the deposit-taking and payment-transmission core of a bank — not just a payments API. Worth an explicit read on Nigerian licensing exposure (money transmission / PSP / MFB territory) before Stage A/B in §9.1 goes live with real merchant funds, not after.

## 12. Roadmap — near-term, ordered

1. Settlement & payout pipeline: sweep merchant wallets → redeem cNGN via the issuer → last-mile payout via Paystack/Flutterwave (closes the "ledger says X, bank account has 0" gap — see §9.1)
2. Source a real cNGN issuer Stellar address (unblocks cNGN QR codes and cNGN-denominated payment requests — see §9.4)
3. Confirmation-depth policy decision + implementation
4. Cleanup: remove or repurpose `src/stellar/mod.rs`

---

*This document should be updated alongside the codebase as decisions in §9 get made and roadmap items in §12 land — treat status claims here the way `README.md`'s status section is treated: nothing marked done that hasn't actually been verified.*
