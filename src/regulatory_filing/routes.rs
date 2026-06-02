//! Route definitions for the regulatory filing pipeline

use axum::{
    routing::{get, post},
    Router,
};

use super::handlers::{
    create_report, get_audit, get_report, list_gateways, record_ack, record_nack, transmit_report,
    FilingState,
};

pub fn router(state: FilingState) -> Router {
    Router::new()
        .route("/gateways", get(list_gateways))
        .route("/", post(create_report))
        .route("/:id", get(get_report))
        .route("/:id/transmit", post(transmit_report))
        .route("/:id/ack", post(record_ack))
        .route("/:id/nack", post(record_nack))
        .route("/:id/audit", get(get_audit))
        .with_state(state)
}
