use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct ApiError {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

pub type ApiResult<T> = Result<T, (StatusCode, Json<ApiError>)>;

pub fn bad_request(message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::BAD_REQUEST, message)
}

/// Same as `bad_request`, but tags the error with the offending field name
/// so clients can map it back to a form input.
pub fn bad_request_field(field: &str, message: &str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiError {
            error: message.into(),
            field: Some(field.into()),
        }),
    )
}

pub fn conflict(message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::CONFLICT, message)
}

/// The caller is authenticated but this credential is not allowed to do this.
/// Distinct from `unauthorized`, which means "prove who you are".
pub fn forbidden(message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::FORBIDDEN, message)
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

pub fn internal<E: std::fmt::Display>(err: E) -> (StatusCode, Json<ApiError>) {
    tracing::error!(error = %err, "internal error");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
}

fn error(status: StatusCode, message: &str) -> (StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            error: message.into(),
            field: None,
        }),
    )
}