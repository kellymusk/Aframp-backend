use std::collections::BTreeMap;

use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct ApiError {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_errors: Option<BTreeMap<String, String>>,
}

pub type ApiResult<T> = Result<T, (StatusCode, Json<ApiError>)>;

pub fn bad_request(message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::BAD_REQUEST, message)
}

pub fn bad_request_with_fields(
    message: &str,
    field_errors: BTreeMap<String, String>,
) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiError {
            error: message.into(),
            field_errors: Some(field_errors),
        }),
    )
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

pub fn unprocessable_entity(message: &str) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::UNPROCESSABLE_ENTITY, message)
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
            field_errors: None,
        }),
    )
}