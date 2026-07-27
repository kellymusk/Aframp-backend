//! Unit tests for the verification service.

use super::service::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a thin in-memory fake that implements only the operations the
/// service uses (get, set, delete, exists, increment, expire, ttl).
///
/// We can't spin up a real Redis in a unit test, so we test the logic
/// that doesn't depend on the cache (format validators, constant-time eq,
/// Redis key builders) and leave the Redis-dependent paths for integration
/// tests.

#[test]
fn otp_redis_key_format() {
    assert_eq!(otp_key("email", "User@Example.COM"), "verify:email:otp:user@example.com");
    assert_eq!(otp_key("phone", "+2348012345678"), "verify:phone:otp:+2348012345678");
}

#[test]
fn rate_redis_key_format() {
    assert_eq!(rate_key("email", "A@B.COM"), "verify:email:rate:a@b.com");
}

#[test]
fn verified_redis_key_format() {
    assert_eq!(
        verified_key("phone", "+2348012345678"),
        "verify:phone:done:+2348012345678"
    );
}

#[test]
fn constant_time_eq_same() {
    assert!(constant_time_eq_pub(b"123456", b"123456"));
}

#[test]
fn constant_time_eq_different() {
    assert!(!constant_time_eq_pub(b"123456", b"123457"));
}

#[test]
fn constant_time_eq_different_lengths() {
    assert!(!constant_time_eq_pub(b"12345", b"123456"));
}

// ---------------------------------------------------------------------------
// Expose private helper for testing
// ---------------------------------------------------------------------------
pub fn constant_time_eq_pub(a: &[u8], b: &[u8]) -> bool {
    super::service::constant_time_eq(a, b)
}
