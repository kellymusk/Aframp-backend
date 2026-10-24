# ADR-001: Custodial per-merchant wallet design

**Status:** Accepted  
**Date:** 2024-11-01  
**Deciders:** Core team

---

## Context

Aframp needs a way to associate Stellar payments with specific merchants and route
the correct funds to the right ledger. Two broad approaches were considered:

**Option A — Single system wallet with memo-based correlation**  
All customer payments flow into one platform-controlled Stellar address. Each
payment request encodes a unique correlation memo; the backend matches incoming
payments to requests by reading that memo from Horizon.

**Option B — Per-merchant wallets**  
Each merchant gets their own Stellar keypair at wallet-creation time. Deposits
to that address are definitively attributed to the owning merchant by address
alone; memo correlation becomes an optional layer for matching a payment to a
specific payment request, not the primary attribution mechanism.

A stub implementation of Option A still exists in `src/stellar/mod.rs` — it
represents the abandoned earlier design and is not compiled into the active
module tree.

---

## Decision

**Option B (per-merchant wallets) was chosen.**

Each call to `POST /wallet/create` generates a real ed25519 keypair via
`src/blockchain/keypair.rs`. The public `G...` address is returned to the
merchant and stored in the `wallets` table. The private `S...` seed is
AES-256-GCM encrypted with `WALLET_ENCRYPTION_KEY` before it ever reaches the
database (`src/blockchain/wallet_crypto.rs`), and the API response never
includes it.

The deposit-detection worker (`src/blockchain/worker.rs`) polls Horizon for
every known wallet address on a configurable timer. Because each address
uniquely identifies a merchant, attribution is unambiguous even with no memo.

---

## Consequences

**Positive:**

- Attribution is O(1): look up the wallet row by destination address.  
  With memo-based correlation, a missed or malformed memo is a permanent loss
  of attribution; with per-merchant addresses, the deposit lands in the right
  place regardless.
- The QR code a customer scans contains the merchant's own Stellar address — no
  indirection. SEP-0007 `web+stellar:pay` URIs point directly to it.
- No single point of failure: losing access to one merchant's encrypted key does
  not affect any other merchant's wallet.

**Negative / open items:**

- Key custody: the platform holds every merchant's encrypted private key.
  Compromise of `WALLET_ENCRYPTION_KEY` exposes all wallet seeds. This was an
  explicit trade-off for the MVP; a future design could move toward
  key-derivation (BIP-32 style) or non-custodial signing.
- Settlement sweep: to pay out from a merchant's wallet, the platform must
  decrypt the seed at payout time and sign the Stellar transaction. A
  settlement/sweep wallet (`STELLAR_SYSTEM_WALLET_ADDRESS`) is reserved for
  this but not yet wired up — funds currently accumulate in individual merchant
  wallets.
- Polling cost scales linearly with merchant count. Acceptable for the MVP;
  a Horizon streaming cursor per wallet is the natural upgrade path.
- The vestigial `src/stellar/mod.rs` stub should be deleted once the per-wallet
  design is fully stable, to avoid confusing future contributors.
