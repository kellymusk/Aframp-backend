//! OTP-based email and phone verification (Issue: empty stub fix).
//!
//! Provides four endpoints:
//!   POST /auth/verify/email/send      — generate & store an email OTP
//!   POST /auth/verify/email/confirm   — validate email OTP, mark verified
//!   POST /auth/verify/phone/send      — generate & store a phone OTP
//!   POST /auth/verify/phone/confirm   — validate phone OTP, mark verified
//!
//! OTPs are:
//!   - 6-digit numeric codes
//!   - stored in Redis with a 10-minute TTL
//!   - rate-limited to 3 send attempts per 10 minutes per address
//!   - invalidated on first successful confirmation (single-use)

pub mod handlers;
pub mod routes;
pub mod service;

#[cfg(test)]
pub mod tests;

pub use routes::verification_router;
pub use service::VerificationService;
