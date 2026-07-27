//! Core verification service — OTP generation, storage, rate-limiting.

use crate::cache::{Cache, RedisCache};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// OTP validity window.
pub const OTP_TTL_SECS: u64 = 600; // 10 minutes

/// How many send attempts are allowed within OTP_TTL_SECS.
pub const MAX_SEND_ATTEMPTS: i64 = 3;

// ---------------------------------------------------------------------------
// Redis key helpers
// ---------------------------------------------------------------------------

pub fn otp_key(channel: &str, address: &str) -> String {
    format!("verify:{}:otp:{}", channel, address.to_lowercase())
}

pub fn rate_key(channel: &str, address: &str) -> String {
    format!("verify:{}:rate:{}", channel, address.to_lowercase())
}

pub fn verified_key(channel: &str, address: &str) -> String {
    format!("verify:{}:done:{}", channel, address.to_lowercase())
}

// ---------------------------------------------------------------------------
// Stored record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtpRecord {
    pub code: String,
    pub address: String,
    pub channel: String, // "email" | "phone"
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum VerificationError {
    #[error("rate limit exceeded — try again in {retry_after_secs} seconds")]
    RateLimitExceeded { retry_after_secs: i64 },

    #[error("invalid or expired OTP")]
    InvalidOtp,

    #[error("address already verified")]
    AlreadyVerified,

    #[error("cache error: {0}")]
    Cache(String),
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct VerificationService {
    cache: RedisCache,
}

impl VerificationService {
    pub fn new(cache: RedisCache) -> Self {
        Self { cache }
    }

    /// Generate and store a new OTP.  Returns the code so the caller can
    /// dispatch it via email / SMS (actual delivery is out of scope here —
    /// logged at INFO level as a placeholder).
    pub async fn send_otp(
        &self,
        channel: &str, // "email" | "phone"
        address: &str,
    ) -> Result<String, VerificationError> {
        // 1. Reject if already verified.
        if self.is_verified(channel, address).await? {
            return Err(VerificationError::AlreadyVerified);
        }

        // 2. Rate-limit: max MAX_SEND_ATTEMPTS per OTP_TTL_SECS window.
        let rate_key = rate_key(channel, address);
        let attempts = self
            .cache
            .increment(&rate_key, 1)
            .await
            .map_err(|e| VerificationError::Cache(e.to_string()))?;

        if attempts == 1 {
            // First attempt in this window — set the expiry.
            self.cache
                .expire(&rate_key, Duration::from_secs(OTP_TTL_SECS))
                .await
                .map_err(|e| VerificationError::Cache(e.to_string()))?;
        }

        if attempts > MAX_SEND_ATTEMPTS {
            let remaining = self
                .cache
                .ttl(&rate_key)
                .await
                .unwrap_or(OTP_TTL_SECS as i64);
            return Err(VerificationError::RateLimitExceeded {
                retry_after_secs: remaining,
            });
        }

        // 3. Generate 6-digit OTP.
        let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32));

        // 4. Store OTP in Redis.
        let record = OtpRecord {
            code: code.clone(),
            address: address.to_lowercase(),
            channel: channel.to_string(),
        };
        self.cache
            .set(
                &otp_key(channel, address),
                &record,
                Some(Duration::from_secs(OTP_TTL_SECS)),
            )
            .await
            .map_err(|e| VerificationError::Cache(e.to_string()))?;

        // 5. Log the OTP (placeholder for real email/SMS dispatch).
        tracing::info!(
            channel = %channel,
            address = %address,
            otp = %code,
            "OTP generated — dispatch via {} provider",
            channel
        );

        Ok(code)
    }

    /// Validate a submitted OTP code.  On success:
    ///   - deletes the OTP (single-use)
    ///   - marks the address as verified (7-day TTL)
    pub async fn confirm_otp(
        &self,
        channel: &str,
        address: &str,
        submitted_code: &str,
    ) -> Result<(), VerificationError> {
        // 1. Reject if already verified.
        if self.is_verified(channel, address).await? {
            return Err(VerificationError::AlreadyVerified);
        }

        // 2. Fetch stored OTP record.
        let key = otp_key(channel, address);
        let record: Option<OtpRecord> = self
            .cache
            .get(&key)
            .await
            .map_err(|e| VerificationError::Cache(e.to_string()))?;

        let record = record.ok_or(VerificationError::InvalidOtp)?;

        // 3. Constant-time compare to prevent timing attacks.
        if !constant_time_eq(record.code.as_bytes(), submitted_code.trim().as_bytes()) {
            return Err(VerificationError::InvalidOtp);
        }

        // 4. Invalidate OTP (single-use).
        self.cache
            .delete(&key)
            .await
            .map_err(|e| VerificationError::Cache(e.to_string()))?;

        // 5. Mark address as verified (7-day TTL).
        self.cache
            .set(
                &verified_key(channel, address),
                &"1".to_string(),
                Some(Duration::from_secs(7 * 24 * 3600)),
            )
            .await
            .map_err(|e| VerificationError::Cache(e.to_string()))?;

        tracing::info!(channel = %channel, address = %address, "Address verified successfully");
        Ok(())
    }

    /// Check whether an address has already been verified.
    pub async fn is_verified(
        &self,
        channel: &str,
        address: &str,
    ) -> Result<bool, VerificationError> {
        self.cache
            .exists(&verified_key(channel, address))
            .await
            .map_err(|e| VerificationError::Cache(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Constant-time byte comparison to prevent timing side-channels.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
