//! Stellar integration: cNGN deposits (offramp), minting (onramp payout),
//! and deposit-to-transaction correlation via memos.
//!
//! # Deprecated — scheduled for removal
//!
//! This module is a vestigial stub from an earlier, abandoned design (a single
//! system wallet with memo-based correlation). It is superseded by the
//! per-merchant wallet design in [`crate::blockchain`] and is not wired into
//! the binary's module tree, so nothing here runs. It is tracked for deletion
//! in <https://github.com/kellymusk/Aframp-backend/issues/970> — do not build
//! anything new on top of it.

#[deprecated(
    since = "0.1.0",
    note = "superseded by the per-wallet design in `crate::blockchain`; scheduled for removal, see issue #970"
)]
pub struct StellarClient {}

#[allow(deprecated)]
impl StellarClient {
    #[deprecated(
        since = "0.1.0",
        note = "superseded by `crate::blockchain::stellar::StellarListener`; scheduled for removal, see issue #970"
    )]
    pub fn new() -> Self {
        Self {}
    }
}
