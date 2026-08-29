//! Admin endpoints for service identity and allowlist management

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info};

// ── State ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ServiceAdminState {
    pub registry: Arc<ServiceRegistry>,
    pub allowlist_repo: Arc<ServiceAllowlistRepository>,
}

// ── Request/Response types ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RegisterServiceRequest {
    pub service_name: String,
    pub allowed_scopes: Vec<String>,
    pub allowed_targets: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RegisterServiceResponse {
    pub service_name: String,
    pub client_id: String,
    pub client_secret: String,
    pub allowed_scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct RotateSecretRequest {
    pub grace_period_secs: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct RotateSecretResponse {
    pub new_client_secret: String,
    pub grace_period_ends: String,
}

#[derive(Debug, Deserialize)]
pub struct SetPermissionRequest {
    pub calling_service: String,
    pub target_endpoint: String,
    pub allowed: bool,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// POST /admin/services/register
pub async fn register_service(
    State(state): State<ServiceAdminState>,
    Json(req): Json<RegisterServiceRequest>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    let registration = ServiceRegistration {
        service_name: req.service_name.clone(),
        allowed_scopes: req.allowed_scopes,
        allowed_targets: req.allowed_targets,
    };

    match state.registry.register_service(registration).await {
        Ok(identity) => {
            info!(service_name = %identity.service_name, "Service registered via admin API");

            Ok((
                StatusCode::CREATED,
                Json(RegisterServiceResponse {
                    service_name: identity.service_name,
                    client_id: identity.client_id,
                    client_secret: identity.client_secret,
                    allowed_scopes: identity.allowed_scopes,
                }),
            ))
        }
        Err(e) => {
            error!(error = %e, "Failed to register service");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "code": "REGISTRATION_FAILED",
                        "message": e.to_string()
                    }
                })),
            ))
        }
    }
}

/// GET /admin/services
pub async fn list_services(
    State(state): State<ServiceAdminState>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    match state.registry.list_services().await {
        Ok(services) => Ok(Json(services)),
        Err(e) => {
            error!(error = %e, "Failed to list services");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "code": "LIST_FAILED",
                        "message": e.to_string()
                    }
                })),
            ))
        }
    }
}

/// GET /admin/services/:service_name
pub async fn get_service(
    State(state): State<ServiceAdminState>,
    Path(service_name): Path<String>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    match state.registry.get_service(&service_name).await {
        Ok(service) => Ok(Json(service)),
        Err(e) => {
            error!(service_name = %service_name, error = %e, "Failed to get service");
            Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": {
                        "code": "SERVICE_NOT_FOUND",
                        "message": e.to_string()
                    }
                })),
            ))
        }
    }
}

/// POST /admin/services/:service_name/rotate-secret
pub async fn rotate_secret(
    State(state): State<ServiceAdminState>,
    Path(service_name): Path<String>,
    Json(req): Json<RotateSecretRequest>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    let grace_period_secs = req.grace_period_secs.unwrap_or(300); // 5 minutes default

    match state
        .registry
        .rotate_secret(&service_name, grace_period_secs)
        .await
    {
        Ok(new_secret) => {
            let grace_period_ends =
                chrono::Utc::now() + chrono::Duration::seconds(grace_period_secs);

            info!(
                service_name = %service_name,
                grace_period_secs = %grace_period_secs,
                "Service secret rotated via admin API"
            );

            Ok(Json(RotateSecretResponse {
                new_client_secret: new_secret,
                grace_period_ends: grace_period_ends.to_rfc3339(),
            }))
        }
        Err(e) => {
            error!(service_name = %service_name, error = %e, "Failed to rotate secret");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "code": "ROTATION_FAILED",
                        "message": e.to_string()
                    }
                })),
            ))
        }
    }
}

/// GET /admin/services/allowlist
pub async fn list_allowlist(
    State(state): State<ServiceAdminState>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    match state.allowlist_repo.list_all().await {
        Ok(entries) => Ok(Json(entries)),
        Err(e) => {
            error!(error = %e, "Failed to list allowlist");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "code": "LIST_FAILED",
                        "message": e.to_string()
                    }
                })),
            ))
        }
    }
}

/// GET /admin/services/allowlist/:service_name
pub async fn list_service_allowlist(
    State(state): State<ServiceAdminState>,
    Path(service_name): Path<String>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    match state.allowlist_repo.list_for_service(&service_name).await {
        Ok(entries) => Ok(Json(entries)),
        Err(e) => {
            error!(service_name = %service_name, error = %e, "Failed to list service allowlist");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "code": "LIST_FAILED",
                        "message": e.to_string()
                    }
                })),
            ))
        }
    }
}

/// POST /admin/services/allowlist/add
pub async fn add_permission(
    State(state): State<ServiceAdminState>,
    Json(req): Json<SetPermissionRequest>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    match state
        .allowlist_repo
        .add_permission(&req.calling_service, &req.target_endpoint)
        .await
    {
        Ok(()) => {
            info!(
                calling_service = %req.calling_service,
                target_endpoint = %req.target_endpoint,
                "Permission added via admin API"
            );
            Ok(StatusCode::NO_CONTENT)
        }
        Err(e) => {
            error!(error = %e, "Failed to add permission");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "code": "ADD_PERMISSION_FAILED",
                        "message": e.to_string()
                    }
                })),
            ))
        }
    }
}

/// POST /admin/services/allowlist/remove
pub async fn remove_permission(
    State(state): State<ServiceAdminState>,
    Json(req): Json<SetPermissionRequest>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    match state
        .allowlist_repo
        .remove_permission(&req.calling_service, &req.target_endpoint)
        .await
    {
        Ok(()) => {
            info!(
                calling_service = %req.calling_service,
                target_endpoint = %req.target_endpoint,
                "Permission removed via admin API"
            );
            Ok(StatusCode::NO_CONTENT)
        }
        Err(e) => {
            error!(error = %e, "Failed to remove permission");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "code": "REMOVE_PERMISSION_FAILED",
                        "message": e.to_string()
                    }
                })),
            ))
        }
    }
}
