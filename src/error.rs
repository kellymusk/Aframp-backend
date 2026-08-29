use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct ApiError {
    pub error: String,
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
        }),
    )
}