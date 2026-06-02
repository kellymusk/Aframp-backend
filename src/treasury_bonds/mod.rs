//! Programmable Sovereign Debt Settlement & Tokenized Treasury Bond Rails — Issue #524
//!
//! Manages fractional tokenized government bond instruments, automated sweep policies,
//! and on-chain minting/liquidation via Stellar/Soroban.

pub mod handlers;
pub mod models;
pub mod repository;
pub mod routes;
pub mod service;

pub use models::{AutomatedSweepPolicy, BondLedgerAllocation, TokenizedBondInstrument};
pub use service::TreasuryBondsService;
