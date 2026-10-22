use axum::extract::State;
use axum::Json;

use crate::auth::extractor::AuthUser;
use crate::error::{internal, not_found, ApiResult, ErrorCode};
use crate::models::Merchant;
use crate::services::users;
use crate::AppState;

/// `GET /merchants/me` — the signed-in user's merchant profile on its own,
/// separate from the auth-identity `GET /me`, so richer business fields can be
/// surfaced here without bloating the identity endpoint.
pub async fn get(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Merchant>> {
    let merchant = users::merchant_by_user(&state.db, auth.user_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::MerchantNotFound, "merchant not found"))?;
    Ok(Json(merchant))
}
