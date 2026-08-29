//! KYB Routes

use super::handlers::{
    get_application, record_decision, start_kyb, submit_document, submit_for_review, verify_registry, KybState,
};
use crate::middleware::rbac::{extract_identity, require_role, ROLE_COMPLIANCE_OFFICER};
use axum::{middleware, routing::{get, post}, Router};
use std::sync::Arc;

pub fn kyb_routes(state: Arc<KybState>) -> Router {
    // Compliance-only sub-router
    let compliance = Router::new()
        .route("/kyb/applications/:id/verify-registry", post(verify_registry))
        .route("/kyb/applications/:id/submit-review", post(submit_for_review))
        .route("/kyb/applications/:id/decision", post(record_decision))
        .route_layer(middleware::from_fn(require_role(ROLE_COMPLIANCE_OFFICER)))
        .route_layer(middleware::from_fn(extract_identity));

    // Open merchant-facing routes
    let merchant = Router::new()
        .route("/kyb/applications", post(start_kyb))
        .route("/kyb/applications/:id/documents", post(submit_document))
        .route("/kyb/applications/:id", get(get_application));

    Router::new()
        .merge(compliance)
        .merge(merchant)
        .with_state(state)
}
