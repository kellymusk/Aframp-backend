use axum::extract::State;
use axum::Json;

use crate::auth::extractor::AuthUser;
use crate::error::{internal, not_found, ApiResult, ErrorCode};
use crate::models::Merchant;
use crate::AppState;

/// `GET /merchants/me` — the signed-in user's merchant profile on its own,
/// separate from the auth-identity `GET /me`. Returns the full `Merchant`
/// model so richer business fields (address, logo, bank details, verification
/// status) can be surfaced here without bloating the identity endpoint.
pub async fn get(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Merchant>> {
    let merchant = sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, email, email_notifications_enabled, unsubscribe_token,
                suspended_at, created_at, updated_at
           FROM merchants WHERE user_id = $1 LIMIT 1",
    )
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or_else(|| not_found(ErrorCode::MerchantNotFound, "merchant not found"))?;

    Ok(Json(merchant))
}
