//! Automated Central Bank Clearing & Interbank Settlement Rail (RTGS Bridge) — Issue #525
//!
//! Two-Phase Commit coordinator, ISO 20022 message processing, HSM-signed payloads,
//! and atomic Stellar cNGN mint/reverse on RTGS settlement confirmation.

pub mod handlers;
pub mod models;
pub mod repository;
pub mod routes;
pub mod service;

pub use models::{
    ClearingHouseLedgerEntry, InterbankReconciliationLog, RtgsSettlementPool, SettlementStatus,
    TwoPcPhase,
};
pub use service::RtgsService;
