//! Payment provider integration module
//!
//! This module provides a unified interface for payment providers (Paystack, Flutterwave, M-Pesa)
//! to support fiat transactions in African markets.

#[cfg(feature = "database")]
pub mod providers;
#[cfg(feature = "database")]
pub mod traits;
#[cfg(feature = "database")]
pub mod types;
