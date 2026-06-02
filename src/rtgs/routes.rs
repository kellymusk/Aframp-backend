//! Route definitions for the RTGS interbank settlement rail

use axum::{
    routing::{get, post},
    Router,
};

use super::handlers::{
    commit_settlement, get_entry, get_logs, hold_settlement, list_pools, prepare_settlement,
    register_pool, reverse_settlement, RtgsState,
};

pub fn router(state: RtgsState) -> Router {
    Router::new()
        .route("/pools", get(list_pools).post(register_pool))
        .route("/settlements/prepare", post(prepare_settlement))
        .route("/settlements/:id", get(get_entry))
        .route("/settlements/:id/commit", post(commit_settlement))
        .route("/settlements/:id/reverse", post(reverse_settlement))
        .route("/settlements/:id/hold", post(hold_settlement))
        .route("/settlements/:id/logs", get(get_logs))
        .with_state(state)
}
