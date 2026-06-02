//! Route definitions for the treasury bonds module

use axum::{
    routing::{get, post},
    Router,
};

use super::handlers::{
    create_allocation, get_allocation, get_instrument, get_sweep_policy, list_instruments,
    liquidate_allocation, register_instrument, upsert_sweep_policy, BondsState,
};

pub fn router(state: BondsState) -> Router {
    Router::new()
        .route("/instruments", get(list_instruments).post(register_instrument))
        .route("/instruments/:id", get(get_instrument))
        .route("/allocations", post(create_allocation))
        .route("/allocations/:id", get(get_allocation))
        .route("/allocations/:id/liquidate", post(liquidate_allocation))
        .route("/sweep-policy", post(upsert_sweep_policy))
        .route("/sweep-policy/:tenant_id", get(get_sweep_policy))
        .with_state(state)
}
