use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct ApiError {
    pub error: String,
    /// Present on `423 Locked` responses: seconds remaining until the account
    /// may attempt login again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

pub type ApiResult<T> = Result<T, (StatusCode, Json<ApiError>)>;

pub fn bad_request(message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::BAD_REQUEST, message)
}

pub fn conflict(message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::CONFLICT, message)
}

pub fn not_found(message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::NOT_FOUND, message)
}

pub fn unsupported_media_type(message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::UNSUPPORTED_MEDIA_TYPE, message)
}

pub fn unauthorized(message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::UNAUTHORIZED, message)
}

pub fn bad_gateway(message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::BAD_GATEWAY, message)
}

/// `423 Locked`: the account is temporarily locked (e.g. too many failed logins).
/// `retry_at` is the instant the lock expires.
pub fn locked(retry_at: chrono::DateTime<chrono::Utc>) -> (StatusCode, Json<ApiError>) {
    let remaining = (retry_at - chrono::Utc::now()).num_seconds().max(0);
    (
        StatusCode::LOCKED,
        Json(ApiError {
            error: "account temporarily locked due to too many failed login attempts".into(),
            retry_after_secs: Some(remaining as u64),
        }),
    )
}

pub fn internal<E: std::fmt::Display>(err: E) -> (StatusCode, Json<ApiError>) {
    tracing::error!(error = %err, "internal error");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
}

fn error(status: StatusCode, message: &str) -> (StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            error: message.into(),
            retry_after_secs: None,
        }),
    )
}