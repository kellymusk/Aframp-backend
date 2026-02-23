//! Backward-compatible export for the payment provider trait.
//! New code should import from `crate::payments::provider`.

pub use crate::payments::provider::PaymentProvider;
