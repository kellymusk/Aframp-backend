//! Compliance Effectiveness Reporting — Route Registration
//!
//! Routes are protected by RBAC: only `compliance_officer` and `finance_director`
//! roles may access these endpoints.

use super::handlers::{generate_report, get_report, list_reports, ComplianceEffectivenessState};
use crate::middleware::rbac::{extract_identity, ROLE_COMPLIANCE_OFFICER, ROLE_FINANCE_DIRECTOR};
use axum::{
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use std::sync::Arc;

/// Allowed roles for compliance reporting endpoints.
const ROLE_FINANCE_DIRECTOR_STR: &str = ROLE_FINANCE_DIRECTOR;

pub fn compliance_effectiveness_routes(state: Arc<ComplianceEffectivenessState>) -> Router {
    // Both compliance_officer and finance_director can access these routes.
    // We use extract_identity + a custom any-of check via require_compliance_role.
    Router::new()
        .route("/compliance/reports", post(generate_report))
        .route("/compliance/reports", get(list_reports))
        .route("/compliance/reports/:id", get(get_report))
        .route_layer(middleware::from_fn(require_compliance_role))
        .route_layer(middleware::from_fn(extract_identity))
        .with_state(state)
}

/// Middleware: allow compliance_officer OR finance_director.
async fn require_compliance_role(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::response::Response> {
    use crate::middleware::rbac::CallerIdentity;
    use axum::{http::StatusCode, Json};
    use serde_json::json;

    let identity = request
        .extensions()
        .get::<CallerIdentity>()
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "code": "UNAUTHORIZED", "message": "Identity not resolved" })),
            )
                .into_response()
        })?;

    let allowed = [ROLE_COMPLIANCE_OFFICER, ROLE_FINANCE_DIRECTOR_STR];
    if !allowed.contains(&identity.role.as_str()) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({
                "code": "FORBIDDEN",
                "message": format!("Role '{}' is not permitted for compliance reporting", identity.role)
            })),
        )
            .into_response());
    }

    Ok(next.run(request).await)
}
