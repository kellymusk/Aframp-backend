use axum::http::StatusCode;
use axum::Json;
use serde::{Serialize, Serializer};

/// Machine-readable error codes returned alongside the human `error` message.
/// Enum variants serialize as `SCREAMING_SNAKE_CASE`, e.g. `INSUFFICIENT_BALANCE`.
/// These are part of the public API contract — see API.md for the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidParameters,
    InvalidAmount,
    InsufficientBalance,
    UnsupportedAsset,
    PayoutFailed,
    EmailTaken,
    InvalidCredentials,
    UserNotFound,
    MerchantNotFound,
    WalletNotFound,
    PaymentRequestNotFound,
    Forbidden,
    PhoneTaken,
    OtpInvalid,
    OtpExpired,
    OtpChallengeNotFound,
    OtpLocked,
    TooManyRequests,
    InternalError,
}

impl Serialize for ErrorCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl ErrorCode {
    pub const fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::InvalidParameters => "INVALID_PARAMETERS",
            ErrorCode::InvalidAmount => "INVALID_AMOUNT",
            ErrorCode::InsufficientBalance => "INSUFFICIENT_BALANCE",
            ErrorCode::UnsupportedAsset => "UNSUPPORTED_ASSET",
            ErrorCode::PayoutFailed => "PAYOUT_FAILED",
            ErrorCode::EmailTaken => "EMAIL_TAKEN",
            ErrorCode::InvalidCredentials => "INVALID_CREDENTIALS",
            ErrorCode::UserNotFound => "USER_NOT_FOUND",
            ErrorCode::MerchantNotFound => "MERCHANT_NOT_FOUND",
            ErrorCode::WalletNotFound => "WALLET_NOT_FOUND",
            ErrorCode::PaymentRequestNotFound => "PAYMENT_REQUEST_NOT_FOUND",
            ErrorCode::Forbidden => "FORBIDDEN",
            ErrorCode::PhoneTaken => "PHONE_TAKEN",
            ErrorCode::OtpInvalid => "OTP_INVALID",
            ErrorCode::OtpExpired => "OTP_EXPIRED",
            ErrorCode::OtpChallengeNotFound => "OTP_CHALLENGE_NOT_FOUND",
            ErrorCode::OtpLocked => "OTP_LOCKED",
            ErrorCode::TooManyRequests => "TOO_MANY_REQUESTS",
            ErrorCode::InternalError => "INTERNAL_ERROR",
        }
    }
}

#[derive(Serialize)]
pub struct ApiError {
    pub error: String,
    pub code: ErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

pub type ApiResult<T> = Result<T, (StatusCode, Json<ApiError>)>;

pub fn bad_request(code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::BAD_REQUEST, code, message)
}

/// Same as `bad_request`, but tags the error with the offending field name
/// so clients can map it back to a form input.
pub fn bad_request_field(field: &str, message: &str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiError {
            error: message.into(),
            code: ErrorCode::InvalidParameters,
            field: Some(field.into()),
        }),
    )
}

pub fn conflict(code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::CONFLICT, code, message)
}

pub fn not_found(code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::NOT_FOUND, code, message)
}

pub fn unsupported_media_type(code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::UNSUPPORTED_MEDIA_TYPE, code, message)
}

pub fn unauthorized(code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::UNAUTHORIZED, code, message)
}

pub fn forbidden(code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::FORBIDDEN, code, message)
}

pub fn bad_gateway(code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::BAD_GATEWAY, code, message)
}

pub fn too_many_requests(code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::TOO_MANY_REQUESTS, code, message)
}

pub fn internal<E: std::fmt::Display>(err: E) -> (StatusCode, Json<ApiError>) {
    tracing::error!(error = %err, "internal error");
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::InternalError,
        "internal server error",
    )
}

fn error(status: StatusCode, code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            error: message.into(),
            code,
            field: None,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize the JSON body of an error response to a pretty string for
    /// snapshotting. This captures the exact wire shape `{ error, code, field? }`
    /// so a refactor that renames fields or changes casing is caught.
    fn body_json(resp: &(StatusCode, Json<ApiError>)) -> String {
        serde_json::to_string_pretty(&resp.1 .0).expect("ApiError serializes")
    }

    fn assert_screaming_snake(code: &str) {
        assert!(
            !code.is_empty()
                && code
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
            "code `{code}` is not SCREAMING_SNAKE_CASE"
        );
    }

    #[test]
    fn snapshot_400_bad_request() {
        let resp = bad_request(ErrorCode::InvalidAmount, "amount must be positive");
        assert_eq!(resp.0, StatusCode::BAD_REQUEST);
        insta::assert_snapshot!(body_json(&resp));
    }

    #[test]
    fn snapshot_401_unauthorized() {
        let resp = unauthorized(ErrorCode::InvalidCredentials, "invalid credentials");
        assert_eq!(resp.0, StatusCode::UNAUTHORIZED);
        insta::assert_snapshot!(body_json(&resp));
    }

    #[test]
    fn snapshot_403_forbidden() {
        let resp = forbidden(ErrorCode::Forbidden, "not allowed");
        assert_eq!(resp.0, StatusCode::FORBIDDEN);
        insta::assert_snapshot!(body_json(&resp));
    }

    #[test]
    fn snapshot_404_not_found() {
        let resp = not_found(ErrorCode::UserNotFound, "user not found");
        assert_eq!(resp.0, StatusCode::NOT_FOUND);
        insta::assert_snapshot!(body_json(&resp));
    }

    #[test]
    fn snapshot_409_conflict() {
        let resp = conflict(ErrorCode::EmailTaken, "email already registered");
        assert_eq!(resp.0, StatusCode::CONFLICT);
        insta::assert_snapshot!(body_json(&resp));
    }

    #[test]
    fn snapshot_415_unsupported_media_type() {
        let resp = unsupported_media_type(ErrorCode::InvalidParameters, "unsupported content type");
        assert_eq!(resp.0, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        insta::assert_snapshot!(body_json(&resp));
    }

    #[test]
    fn snapshot_422_validation_error() {
        let resp = bad_request_field("email", "must be a valid email address");
        assert_eq!(resp.0, StatusCode::BAD_REQUEST);
        insta::assert_snapshot!(body_json(&resp));
    }

    #[test]
    fn snapshot_429_too_many_requests() {
        let resp = too_many_requests(ErrorCode::TooManyRequests, "rate limit exceeded");
        assert_eq!(resp.0, StatusCode::TOO_MANY_REQUESTS);
        insta::assert_snapshot!(body_json(&resp));
    }

    #[test]
    fn snapshot_500_internal_error() {
        let resp = internal("database connection lost");
        assert_eq!(resp.0, StatusCode::INTERNAL_SERVER_ERROR);
        insta::assert_snapshot!(body_json(&resp));
    }

    #[test]
    fn snapshot_502_bad_gateway() {
        let resp = bad_gateway(ErrorCode::PayoutFailed, "upstream provider unavailable");
        assert_eq!(resp.0, StatusCode::BAD_GATEWAY);
        insta::assert_snapshot!(body_json(&resp));
    }

    #[test]
    fn all_error_codes_are_screaming_snake_case() {
        let codes = [
            ErrorCode::InvalidParameters,
            ErrorCode::InvalidAmount,
            ErrorCode::InsufficientBalance,
            ErrorCode::UnsupportedAsset,
            ErrorCode::PayoutFailed,
            ErrorCode::EmailTaken,
            ErrorCode::InvalidCredentials,
            ErrorCode::UserNotFound,
            ErrorCode::MerchantNotFound,
            ErrorCode::WalletNotFound,
            ErrorCode::PaymentRequestNotFound,
            ErrorCode::Forbidden,
            ErrorCode::PhoneTaken,
            ErrorCode::OtpInvalid,
            ErrorCode::OtpExpired,
            ErrorCode::OtpChallengeNotFound,
            ErrorCode::OtpLocked,
            ErrorCode::TooManyRequests,
            ErrorCode::InternalError,
        ];
        for code in codes {
            assert_screaming_snake(code.as_str());
        }
    }

    #[test]
    fn field_only_present_in_validation_errors() {
        // Validation error carries the offending field.
        let validation = bad_request_field("email", "must be a valid email address");
        let json = serde_json::to_value(&validation.1 .0).unwrap();
        assert_eq!(json.get("field").and_then(|v| v.as_str()), Some("email"));

        // Non-validation errors omit `field` entirely.
        let plain = bad_request(ErrorCode::InvalidAmount, "amount must be positive");
        let json = serde_json::to_value(&plain.1 .0).unwrap();
        assert!(json.get("field").is_none());
    }
}