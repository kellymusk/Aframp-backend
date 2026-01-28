//! Payment provider implementations
//!
//! Concrete implementations of the PaymentProvider trait for different providers.

#[cfg(feature = "database")]
pub mod paystack;

#[cfg(feature = "database")]
pub use paystack::PaystackProvider;
