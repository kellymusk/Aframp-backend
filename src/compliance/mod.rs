//! Enhanced Sanctions Screening Engine — Issue #495.
//!
//! Extends the existing `sanctions` module with:
//! - Compliance watchlist DB management
//! - Behavioral pattern analytics (smurfing, velocity anomalies)
//! - Bloom filter cache for fast non-sanctioned actor bypass
//! - Watchlist sync daemon
//! - HELD_FOR_REVIEW transaction interceptor

pub mod models;
pub mod repository;

pub use models::{BehavioralRiskRecord, ComplianceWatchlistEntry, SanctionsMatchRecord};
pub use repository::ComplianceRepository;
