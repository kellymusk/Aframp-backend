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
            ErrorCode::InternalError => "INTERNAL_ERROR",
        }
    }
}

#[derive(Serialize)]
pub struct ApiError {
    pub error: String,
    pub code: ErrorCode,
}

pub type ApiResult<T> = Result<T, (StatusCode, Json<ApiError>)>;

pub fn bad_request(code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::BAD_REQUEST, code, message)
}

pub fn conflict(code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::CONFLICT, code, message)
}

pub fn not_found(code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::NOT_FOUND, code, message)
}

pub fn unauthorized(code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::UNAUTHORIZED, code, message)
}

pub fn bad_gateway(code: ErrorCode, message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::BAD_GATEWAY, code, message)
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
        }),
    )
}