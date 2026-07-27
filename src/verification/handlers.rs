//! Axum HTTP handlers for the verification endpoints.

use crate::verification::service::{VerificationError, VerificationService};
use axum::{extract::State, http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct VerificationState {
    pub service: Arc<VerificationService>,
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SendRequest {
    /// Email address or phone number to verify.
    pub address: String,
}

#[derive(Debug, Deserialize)]
pub struct ConfirmRequest {
    pub address: String,
    pub code: String,
}

#[derive(Debug, Serialize)]
pub struct SendResponse {
    pub success: bool,
    pub message: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ConfirmResponse {
    pub success: bool,
    pub message: &'static str,
    pub verified: bool,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub success: bool,
    pub code: &'static str,
    pub message: String,
}

fn verification_error(e: VerificationError) -> (StatusCode, Json<ErrorBody>) {
    let (status, code) = match &e {
        VerificationError::RateLimitExceeded { .. } => {
            (StatusCode::TOO_MANY_REQUESTS, "RATE_LIMIT_EXCEEDED")
        }
        VerificationError::InvalidOtp => (StatusCode::UNPROCESSABLE_ENTITY, "INVALID_OTP"),
        VerificationError::AlreadyVerified => (StatusCode::CONFLICT, "ALREADY_VERIFIED"),
        VerificationError::Cache(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
    };
    (
        status,
        Json(ErrorBody {
            success: false,
            code,
            message: e.to_string(),
        }),
    )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /auth/verify/email/send
pub async fn send_email_otp(
    State(state): State<Arc<VerificationState>>,
    Json(req): Json<SendRequest>,
) -> Result<Json<SendResponse>, (StatusCode, Json<ErrorBody>)> {
    let address = req.address.trim().to_lowercase();
    if !is_valid_email(&address) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                success: false,
                code: "INVALID_EMAIL",
                message: "Provided address is not a valid email".to_string(),
            }),
        ));
    }

    state
        .service
        .send_otp("email", &address)
        .await
        .map(|_| {
            Json(SendResponse {
                success: true,
                message: "Verification code sent to your email address",
            })
        })
        .map_err(verification_error)
}

/// POST /auth/verify/email/confirm
pub async fn confirm_email_otp(
    State(state): State<Arc<VerificationState>>,
    Json(req): Json<ConfirmRequest>,
) -> Result<Json<ConfirmResponse>, (StatusCode, Json<ErrorBody>)> {
    let address = req.address.trim().to_lowercase();

    state
        .service
        .confirm_otp("email", &address, &req.code)
        .await
        .map(|_| {
            Json(ConfirmResponse {
                success: true,
                message: "Email address verified successfully",
                verified: true,
            })
        })
        .map_err(verification_error)
}

/// POST /auth/verify/phone/send
pub async fn send_phone_otp(
    State(state): State<Arc<VerificationState>>,
    Json(req): Json<SendRequest>,
) -> Result<Json<SendResponse>, (StatusCode, Json<ErrorBody>)> {
    let phone = req.address.trim().to_string();
    if !is_valid_phone(&phone) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                success: false,
                code: "INVALID_PHONE",
                message: "Provided phone number is not valid (E.164 format expected, e.g. +2348012345678)".to_string(),
            }),
        ));
    }

    state
        .service
        .send_otp("phone", &phone)
        .await
        .map(|_| {
            Json(SendResponse {
                success: true,
                message: "Verification code sent to your phone number",
            })
        })
        .map_err(verification_error)
}

/// POST /auth/verify/phone/confirm
pub async fn confirm_phone_otp(
    State(state): State<Arc<VerificationState>>,
    Json(req): Json<ConfirmRequest>,
) -> Result<Json<ConfirmResponse>, (StatusCode, Json<ErrorBody>)> {
    let phone = req.address.trim().to_string();

    state
        .service
        .confirm_otp("phone", &phone, &req.code)
        .await
        .map(|_| {
            Json(ConfirmResponse {
                success: true,
                message: "Phone number verified successfully",
                verified: true,
            })
        })
        .map_err(verification_error)
}

// ---------------------------------------------------------------------------
// Simple format validators
// ---------------------------------------------------------------------------

fn is_valid_email(s: &str) -> bool {
    // Minimal RFC-5321 shape check: local@domain.tld
    let parts: Vec<&str> = s.splitn(2, '@').collect();
    if parts.len() != 2 {
        return false;
    }
    let (local, domain) = (parts[0], parts[1]);
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

fn is_valid_phone(s: &str) -> bool {
    // E.164: + followed by 7–15 digits
    let s = s.trim();
    if !s.starts_with('+') {
        return false;
    }
    let digits: &str = &s[1..];
    digits.len() >= 7 && digits.len() <= 15 && digits.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_validation() {
        assert!(is_valid_email("user@example.com"));
        assert!(is_valid_email("u+tag@sub.domain.io"));
        assert!(!is_valid_email("notanemail"));
        assert!(!is_valid_email("@nodomain.com"));
        assert!(!is_valid_email("noatsign"));
    }

    #[test]
    fn phone_validation() {
        assert!(is_valid_phone("+2348012345678"));
        assert!(is_valid_phone("+447911123456"));
        assert!(!is_valid_phone("08012345678")); // no +
        assert!(!is_valid_phone("+123"));        // too short
        assert!(!is_valid_phone("+12345678901234567")); // too long
    }
}
