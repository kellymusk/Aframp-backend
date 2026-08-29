use axum::extract::State;
use axum::Json;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, ApiResult};
use crate::models::Balance;
use crate::services::balances;
use crate::AppState;

pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<Vec<Balance>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request("no merchant associated with this account"))?;
    let balances = balances::get_balances(&state.db, merchant_id)
        .await
        .map_err(internal)?;
    Ok(Json(balances))
}